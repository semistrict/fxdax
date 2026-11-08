// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! What the automounter does for each of the kernel's requests, whatever the kernel and the
//! filesystem: a lookup's name mounted, from a volume there or one the sandbox manager attaches,
//! or failed with nothing left behind; an idle mount unmounted.

use crate::upcall::Answer;
use std::io;
use std::path::{Path, PathBuf};

/// The volumes of the guest's filesystem.
pub trait Volumes {
    /// Mounts the volume `name` at `target`, a directory: false if there is no such volume.
    fn mount(&self, name: &str, target: &Path) -> io::Result<bool>;
    /// Makes the volume `name`, over the base `manifest` describes.
    fn import(&self, name: &str, manifest: &[u8]) -> io::Result<()>;
    /// Unmounts what is mounted at `target`, which closes its volume.
    fn unmount(&self, target: &Path) -> io::Result<()>;
}

/// The sandbox manager, which attaches bases to the sandbox's store.
pub trait Attach {
    /// Asks it to attach the base `name`.
    fn attach(&mut self, name: &str) -> io::Result<Answer>;
}

/// An automounter of the volumes `volumes` under the directory `root`, attached by `attach`.
pub struct Automounter<V, A> {
    pub root: PathBuf,
    pub volumes: V,
    pub attach: A,
}

impl<V: Volumes, A: Attach> Automounter<V, A> {
    /// Mounts `name` at its directory under the root, which it makes: the volume of that name,
    /// made first, of the base the sandbox manager attaches, if there is none. Whether it did; if
    /// not, nothing is left mounted, nor made, and why is logged.
    pub fn missing(&mut self, name: &str) -> bool {
        let target = self.root.join(name);
        match self.mount(name, &target) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("fxfs-automount: {name}: {error}");
                if let Err(error) = std::fs::remove_dir(&target)
                    && error.kind() != io::ErrorKind::NotFound
                {
                    eprintln!("fxfs-automount: removing {}: {error}", target.display());
                }
                false
            }
        }
    }

    fn mount(&mut self, name: &str, target: &Path) -> io::Result<()> {
        if !crate::valid_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no volume's name"));
        }
        match std::fs::create_dir(target) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        if self.volumes.mount(name, target)? {
            return Ok(());
        }
        match self.attach.attach(name)? {
            Answer::Refused(why) => Err(io::Error::other(format!("refused: {why}"))),
            Answer::Manifest(manifest) => {
                self.volumes.import(name, &manifest)?;
                match self.volumes.mount(name, target)? {
                    true => Ok(()),
                    false => Err(io::Error::other("imported, and still no volume")),
                }
            }
        }
    }

    /// Unmounts `name`, idle, and removes its directory: whether it did.
    pub fn expire(&mut self, name: &str) -> bool {
        let target = self.root.join(name);
        if let Err(error) = self.volumes.unmount(&target) {
            eprintln!("fxfs-automount: unmounting {name}: {error}");
            return false;
        }
        if let Err(error) = std::fs::remove_dir(&target) {
            eprintln!("fxfs-automount: removing {}: {error}", target.display());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};

    /// Volumes in memory: those there, by name, with the manifest each was made of, and where
    /// each is mounted.
    #[derive(Default)]
    struct Fake {
        volumes: RefCell<BTreeMap<String, Vec<u8>>>,
        mounted: RefCell<BTreeMap<PathBuf, String>>,
        busy: RefCell<BTreeSet<PathBuf>>,
    }

    impl Volumes for &Fake {
        fn mount(&self, name: &str, target: &Path) -> io::Result<bool> {
            assert!(target.is_dir(), "{} is no directory", target.display());
            if !self.volumes.borrow().contains_key(name) {
                return Ok(false);
            }
            self.mounted.borrow_mut().insert(target.to_owned(), name.to_owned());
            Ok(true)
        }
        fn import(&self, name: &str, manifest: &[u8]) -> io::Result<()> {
            if manifest == b"bad" {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            self.volumes.borrow_mut().insert(name.to_owned(), manifest.to_owned());
            Ok(())
        }
        fn unmount(&self, target: &Path) -> io::Result<()> {
            if self.busy.borrow().contains(target) {
                return Err(io::Error::from_raw_os_error(libc::EBUSY));
            }
            self.mounted.borrow_mut().remove(target).map(drop).ok_or(io::ErrorKind::NotFound.into())
        }
    }

    /// A sandbox manager attaching what it has, by name, and the names asked for.
    struct Manager {
        bases: BTreeMap<&'static str, &'static [u8]>,
        asked: Vec<String>,
    }

    impl Attach for &mut Manager {
        fn attach(&mut self, name: &str) -> io::Result<Answer> {
            self.asked.push(name.to_owned());
            Ok(match self.bases.get(name) {
                Some(manifest) => Answer::Manifest(manifest.to_vec()),
                None => Answer::Refused(format!("{name} is not attached here")),
            })
        }
    }

    fn manager() -> Manager {
        let bases = BTreeMap::from([("linux", &b"linux's"[..]), ("bad", b"bad")]);
        Manager { bases, asked: Vec::new() }
    }

    /// A name with no volume is attached, imported and mounted at its directory; again, once
    /// expired, it mounts with no ask.
    #[test]
    fn names_are_attached_once_and_mounted() {
        let (dir, fake, mut manager) = (tempfile::tempdir().unwrap(), Fake::default(), manager());
        let root = dir.path().to_owned();
        let mut automounter =
            Automounter { root: root.clone(), volumes: &fake, attach: &mut manager };
        assert!(automounter.missing("linux"));
        assert_eq!(fake.volumes.borrow()["linux"], b"linux's");
        assert_eq!(fake.mounted.borrow()[&root.join("linux")], "linux");

        assert!(automounter.expire("linux"));
        assert!(fake.mounted.borrow().is_empty());
        assert!(!root.join("linux").exists(), "the expired mount's directory stayed");
        assert!(automounter.missing("linux"));
        assert_eq!(fake.mounted.borrow()[&root.join("linux")], "linux");
        drop(automounter);
        assert_eq!(manager.asked, ["linux"]);
    }

    /// A volume there already, made before a reboot, mounts with no ask.
    #[test]
    fn volumes_there_mount_unasked() {
        let (dir, fake, mut manager) = (tempfile::tempdir().unwrap(), Fake::default(), manager());
        let root = dir.path().to_owned();
        fake.volumes.borrow_mut().insert("made-before".into(), b"m".to_vec());
        let mut automounter =
            Automounter { root: root.clone(), volumes: &fake, attach: &mut manager };
        assert!(automounter.missing("made-before"));
        drop(automounter);
        assert!(manager.asked.is_empty());
    }

    /// What the manager refuses, a manifest that doesn't import, and a name no volume may have
    /// fail, leaving nothing mounted, made or asked of the manager for the last.
    #[test]
    fn refusals_leave_nothing() {
        let (dir, fake, mut manager) = (tempfile::tempdir().unwrap(), Fake::default(), manager());
        let root = dir.path().to_owned();
        let mut automounter =
            Automounter { root: root.clone(), volumes: &fake, attach: &mut manager };
        for name in ["refused", "bad", ".hidden", "default"] {
            assert!(!automounter.missing(name), "{name}");
            assert!(!root.join(name).exists(), "{name}'s directory stayed");
        }
        assert!(fake.mounted.borrow().is_empty());
        assert!(fake.volumes.borrow().is_empty());
        drop(automounter);
        assert_eq!(manager.asked, ["refused", "bad"]);
    }

    /// A mount in use isn't expired: it stays, with its directory.
    #[test]
    fn busy_mounts_stay() {
        let (dir, fake, mut manager) = (tempfile::tempdir().unwrap(), Fake::default(), manager());
        let root = dir.path().to_owned();
        let mut automounter =
            Automounter { root: root.clone(), volumes: &fake, attach: &mut manager };
        assert!(automounter.missing("linux"));
        fake.busy.borrow_mut().insert(root.join("linux"));
        assert!(!automounter.expire("linux"));
        assert_eq!(fake.mounted.borrow()[&root.join("linux")], "linux");
        assert!(root.join("linux").is_dir());
    }
}
