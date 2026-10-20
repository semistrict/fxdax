// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Container images as base stores (fxdax's TASK-39.5): the host builds a store from an OCI image
//! of two layers, as `fxdaxctl import` does, and forks it; a guest boots on the fork, and finds the
//! image's files, with their contents, owners, permissions, times, hard links, symlinks, device
//! nodes and extended attributes, and the upper layer's whiteouts applied. It writes to the fork,
//! which the base doesn't see.
//!
//! fxfs-guest-import-dax: the same with a DAX window, which the guest maps the files' data from.

use macros::{guest, host};

pub struct TestFxfsGuestImport {
    /// Whether the device has a DAX window.
    #[cfg_attr(feature = "guest", allow(dead_code))]
    pub dax: bool,
}

/// When the image's entries were made, unless they say otherwise: 2020-09-13.
const MTIME: u64 = 1_600_000_000;
const PASSWD: &[u8] = b"root:x:0:0:root:/root:/bin/sh\nuser:x:1000:1000::/home/user:/bin/sh\n";
const NOTES: &[u8] = b"the user's notes\n";
const MOTD: &[u8] = b"replaced by the upper layer\n";
/// `security.capability`'s value of `cap_net_raw+ep` (VFS_CAP_REVISION_2).
const CAPABILITY: [u8; 20] = [1, 0, 0, 2, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
/// The file the guest writes to the fork.
const WRITTEN: &str = "written-by-the-guest";

/// The large file's contents: several of the guest's chunks of reads, in a pattern no block
/// repeats.
fn large() -> Vec<u8> {
    (0..(3 << 20) + 4321).map(|i: usize| (i % 251) as u8 ^ (i / 4096) as u8).collect()
}

/// A file with a hole: data, a megabyte of zeros, and data.
fn holey() -> Vec<u8> {
    [&b"before the hole"[..], &[0; 1 << 20], b"after the hole"].concat()
}

#[host]
mod host {
    use super::*;
    use crate::common::{fxdax_guest_should_run, setup_rootfs};
    use crate::vm::Vm;
    use crate::{ShouldRun, Test, TestOutcome, TestSetup};
    use futures::executor::block_on;
    use fxdax::import::testing::{Entry, Kind, gzip, layer, oci_layout, opaque, whiteout};
    use fxdax::{Errno, Filesystem, Inode, Storage};
    use std::path::Path;

    const CAPACITY: u64 = 256 << 20;
    const WINDOW: u64 = 64 << 20;

    /// The image's layers, bottom first: the lower gzipped, as images' usually are.
    fn layers() -> anyhow::Result<Vec<Vec<u8>>> {
        let large = large();
        let holey = holey();
        let lower = layer(&[
            Entry::new("etc", Kind::Dir).mtime(MTIME + 1),
            Entry::new("etc/passwd", Kind::File(PASSWD)).mtime(MTIME + 2),
            Entry::new("etc/motd", Kind::File(b"the lower layer's\n")),
            Entry::new("tmp", Kind::Dir).perm(0o1777).mtime(MTIME),
            Entry::new("usr/bin/sudo", Kind::File(b"#!/bin/false\n")).perm(0o4755).mtime(MTIME),
            Entry::new("bin/ping", Kind::File(b"#!/bin/false\n"))
                .perm(0o755)
                .mtime(MTIME)
                .xattrs(&[("security.capability", &CAPABILITY), ("user.origin", b"lower")]),
            Entry::new("home/user", Kind::Dir).perm(0o700).owner(1000, 1000).mtime(MTIME),
            Entry::new("home/user/notes", Kind::File(NOTES))
                .perm(0o600)
                .owner(1000, 100)
                .mtime(MTIME),
            Entry::new("home/user/link", Kind::Symlink("notes")).owner(1000, 100).mtime(MTIME),
            Entry::new("home/user/hard", Kind::HardLink("home/user/notes")),
            Entry::new("dev/null", Kind::CharDevice(1, 3)).perm(0o666).mtime(MTIME),
            Entry::new("dev/sda1", Kind::BlockDevice(8, 1)).perm(0o660).owner(0, 6).mtime(MTIME),
            Entry::new("run/fifo", Kind::Fifo).perm(0o620).mtime(MTIME),
            Entry::new("data/large", Kind::File(&large)).mtime(MTIME),
            Entry::new("data/holey", Kind::File(&holey)).mtime(MTIME),
            Entry::new("gone", Kind::File(b"whited out\n")),
            Entry::new("cache/stale", Kind::File(b"stale\n")),
        ])?;
        let upper = layer(&[
            Entry::new(&whiteout("gone"), Kind::File(b"")),
            Entry::new(&opaque("cache"), Kind::File(b"")),
            Entry::new("cache/fresh", Kind::File(b"fresh\n")).mtime(MTIME),
            Entry::new("etc/motd", Kind::File(MOTD)).owner(1, 2).mtime(MTIME + 3),
        ])?;
        Ok(vec![gzip(&lower)?, upper])
    }

    /// Whether the store at `path` has the file the guest writes.
    fn has_written(path: &Path) -> anyhow::Result<bool> {
        block_on(async {
            let fs = Filesystem::mount(Storage::open(path)?).await?;
            let found = match fs.lookup(Inode::ROOT, WRITTEN.as_bytes()).await {
                Ok(_) => true,
                Err(Errno::ENOENT) => false,
                Err(errno) => anyhow::bail!("looking up {WRITTEN}: {errno}"),
            };
            fs.close().await?;
            Ok(found)
        })
    }

    impl Test for TestFxfsGuestImport {
        fn should_run(&self) -> ShouldRun {
            fxdax_guest_should_run()
        }

        fn start_vm(self: Box<Self>, test_setup: TestSetup) -> anyhow::Result<()> {
            let dir = &test_setup.tmp_dir;
            let layout = dir.join("image");
            oci_layout(&layout, "fxdax/test:import", &layers()?)?;
            let base = dir.join("base");
            block_on(fxdax::import::store(&base, CAPACITY, &layout, Some("fxdax/test:import")))?;
            // A branch of each case's own: one run's daemon keeps both cases' branches in its
            // bucket, where the second's would find the first's moved on (stale).
            let branch = if self.dax { 0x1406 } else { 0x1405 };
            fxdax::ExtentStore::open(&base)?.fork(dir.join("extents"), branch)?;

            let mut vm = Vm::new(2, 512)?;
            vm.guest_kernel()?;
            let window = if self.dax { WINDOW } else { 0 };
            vm.extent_store("extents", &dir.join("extents"), CAPACITY, window)?;
            vm.enter(&setup_rootfs(&test_setup)?, &test_setup.test_case)
        }

        fn check(self: Box<Self>, stdout: Vec<u8>, test_setup: TestSetup) -> TestOutcome {
            let output = String::from_utf8(stdout).unwrap();
            if output != "OK\n" {
                return TestOutcome::Fail(format!("expected exactly \"OK\\n\", got {output:?}"));
            }
            // The guest wrote to the fork, not the base.
            let dir = &test_setup.tmp_dir;
            match (has_written(&dir.join("extents")), has_written(&dir.join("base"))) {
                (Ok(true), Ok(false)) => TestOutcome::Pass,
                outcome => TestOutcome::Fail(format!(
                    "expected the fork to have {WRITTEN} and the base not to, got {outcome:?}"
                )),
            }
        }
    }
}

#[guest]
mod guest {
    use super::*;
    use crate::Test;

    use nix::libc;
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    const MNT: &str = "/mnt/fxfs";

    fn mount() {
        let (source, target, fstype) = (
            CString::new("extents").unwrap(),
            CString::new(MNT).unwrap(),
            CString::new("fxfs").unwrap(),
        );
        let ret = unsafe {
            libc::mount(source.as_ptr(), target.as_ptr(), fstype.as_ptr(), 0, std::ptr::null())
        };
        assert_eq!(ret, 0, "mount: {}", std::io::Error::last_os_error());
    }

    fn unmount() {
        let target = CString::new(MNT).unwrap();
        assert_eq!(unsafe { libc::umount(target.as_ptr()) }, 0, "umount");
    }

    fn path(path: &str) -> String {
        format!("{MNT}/{path}")
    }

    /// The type and permission bits, owner and modification time of the node at `path`, not
    /// following a symlink.
    fn stat(path: &str) -> (u32, u32, u32, i64) {
        let metadata = fs::symlink_metadata(self::path(path)).unwrap();
        (metadata.mode(), metadata.uid(), metadata.gid(), metadata.mtime())
    }

    /// The value of the extended attribute `name` of the node at `path`.
    fn xattr(path: &str, name: &str) -> Vec<u8> {
        let (path, name) = (CString::new(self::path(path)).unwrap(), CString::new(name).unwrap());
        let mut value = vec![0u8; 256];
        let len = unsafe {
            libc::lgetxattr(path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), value.len())
        };
        assert!(len >= 0, "lgetxattr {name:?}: {}", std::io::Error::last_os_error());
        value.truncate(len as usize);
        value
    }

    /// The names of the entries of the directory at `path`, sorted.
    fn names(path: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self::path(path))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    impl Test for TestFxfsGuestImport {
        fn in_guest(self: Box<Self>) {
            fs::create_dir_all(MNT).unwrap();
            mount();
            let mtime = MTIME as i64;

            assert_eq!(stat("etc"), (0o40755, 0, 0, mtime + 1));
            assert_eq!(stat("etc/passwd"), (0o100644, 0, 0, mtime + 2));
            assert_eq!(fs::read(path("etc/passwd")).unwrap(), PASSWD);
            assert_eq!(stat("tmp"), (0o41777, 0, 0, mtime));
            assert_eq!(stat("usr/bin/sudo"), (0o104755, 0, 0, mtime));
            // Directories no entry made: root's, 0755, at the epoch.
            assert_eq!(stat("usr"), (0o40755, 0, 0, 0));

            assert_eq!(xattr("bin/ping", "security.capability"), CAPABILITY);
            assert_eq!(xattr("bin/ping", "user.origin"), b"lower");

            assert_eq!(stat("home/user"), (0o40700, 1000, 1000, mtime));
            assert_eq!(stat("home/user/notes"), (0o100600, 1000, 100, mtime));
            let notes = fs::metadata(path("home/user/notes")).unwrap();
            let hard = fs::metadata(path("home/user/hard")).unwrap();
            assert_eq!((hard.ino(), hard.nlink()), (notes.ino(), 2));
            assert_eq!(fs::read(path("home/user/hard")).unwrap(), NOTES);
            assert_eq!(stat("home/user/link"), (0o120777, 1000, 100, mtime));
            assert_eq!(fs::read_link(path("home/user/link")).unwrap().to_str(), Some("notes"));
            assert_eq!(fs::read(path("home/user/link")).unwrap(), NOTES);

            let null = fs::symlink_metadata(path("dev/null")).unwrap();
            assert!(null.file_type().is_char_device());
            assert_eq!(
                (null.mode() & 0o7777, libc::major(null.rdev()), libc::minor(null.rdev())),
                (0o666, 1, 3)
            );
            let sda1 = fs::symlink_metadata(path("dev/sda1")).unwrap();
            assert!(sda1.file_type().is_block_device());
            assert_eq!((sda1.gid(), libc::major(sda1.rdev()), libc::minor(sda1.rdev())), (6, 8, 1));
            assert_eq!(stat("run/fifo"), (0o10620, 0, 0, mtime));

            assert_eq!(fs::read(path("data/large")).unwrap(), large());
            assert_eq!(fs::read(path("data/holey")).unwrap(), holey());

            // The upper layer: a file replaced, one whited out, a directory made opaque.
            assert_eq!(stat("etc/motd"), (0o100644, 1, 2, mtime + 3));
            assert_eq!(fs::read(path("etc/motd")).unwrap(), MOTD);
            assert!(!fs::exists(path("gone")).unwrap());
            assert_eq!(names("cache"), ["fresh"]);
            assert_eq!(
                names(""),
                ["bin", "cache", "data", "dev", "etc", "home", "run", "tmp", "usr"]
            );

            // The fork is the guest's to write.
            fs::write(path(WRITTEN), b"the fork's own").unwrap();
            unmount();
            mount();
            assert_eq!(fs::read(path(WRITTEN)).unwrap(), b"the fork's own");
            unmount();
            println!("OK");
        }
    }
}
