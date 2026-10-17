// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stores in directories of the host's, as fxdax's images and extentd's stores are: a store's own
//! directory opens its first branch; a fork's directory is a handle, naming the store and the
//! branch, which opens that branch. A process shares a store between the branches of it it opens.

use crate::dir::{Dir, File as _, HostDir};
use crate::layout::{DESCRIPTION, Description};
use crate::lineage::Lineage;
use crate::store::{Options, Store};
use anyhow::{Context as _, Error, anyhow, ensure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

/// The first line of a handle's `store` file.
const HANDLE: &str = "fxdax extent store handle";

/// The stores this process has open, by their directories' canonical paths.
static OPEN: Mutex<Vec<(PathBuf, Weak<Lineage<HostDir>>)>> = Mutex::new(Vec::new());

/// The store at `path`, which its directory holds or a handle names, and the branch of it to open.
fn resolve(path: &Path) -> Result<(PathBuf, Option<u64>), Error> {
    let text = std::fs::read_to_string(path.join(DESCRIPTION))
        .with_context(|| format!("reading {}'s description", path.display()))?;
    let mut lines = text.lines();
    if lines.next() != Some(HANDLE) {
        return Ok((path.canonicalize()?, None));
    }
    let fields: HashMap<&str, &str> = lines.filter_map(|line| line.split_once(' ')).collect();
    let store = fields.get("store").ok_or_else(|| anyhow!("{}: no store", path.display()))?;
    let branch = fields.get("branch").ok_or_else(|| anyhow!("{}: no branch", path.display()))?;
    let branch = branch.parse().with_context(|| format!("{}: bad branch", path.display()))?;
    Ok((PathBuf::from(store).canonicalize()?, Some(branch)))
}

/// The store whose directory is at `path`, which this process shares, opened if it isn't, its
/// extents' files bypassing the host's page cache if `direct` and the host's file system allows.
fn lineage(path: &Path, direct: bool, options: Options) -> Result<Arc<Lineage<HostDir>>, Error> {
    let mut open = OPEN.lock().unwrap();
    open.retain(|(_, lineage)| lineage.strong_count() > 0);
    if let Some(lineage) =
        open.iter().find(|(open, _)| open == path).and_then(|(_, lineage)| lineage.upgrade())
    {
        return Ok(lineage);
    }
    let lineage = Lineage::open(HostDir::new(path, direct)?, options)?;
    open.push((path.to_owned(), Arc::downgrade(&lineage)));
    Ok(lineage)
}

/// Opens the store at `path`: its first branch, or, if `path` is a fork's handle, that fork's.
pub fn open_host(path: &Path, direct: bool, options: Options) -> Result<Store<HostDir>, Error> {
    let (store, branch) = resolve(path)?;
    let lineage = lineage(&store, direct, options)?;
    let branch = branch.unwrap_or_else(|| lineage.first());
    lineage.open_branch(branch)
}

/// Makes a new store at `path`, which must be an empty directory, as `description` describes, and
/// opens it.
pub fn create_host(
    path: &Path,
    description: Description,
    direct: bool,
    options: Options,
) -> Result<Store<HostDir>, Error> {
    let lineage = Lineage::create(HostDir::new(path, direct)?, description, options)?;
    OPEN.lock().unwrap().push((path.canonicalize()?, Arc::downgrade(&lineage)));
    lineage.open_branch(description.branch)
}

/// Forks `store`'s branch ([`Store::fork`]) to `child`, a new branch whose handle is a new
/// directory at `path`, which opening `path` opens. The store's directory must stay where it is
/// for as long as the fork is.
pub fn fork_host(store: &Store<HostDir>, path: &Path, child: u64) -> Result<(), Error> {
    std::fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    let handled = || -> Result<(), Error> {
        let text = format!(
            "{HANDLE}\nstore {}\nbranch {child}\n",
            store.dir().path().canonicalize()?.display()
        );
        let file = HostDir::new(path, false)?.create(DESCRIPTION, false)?;
        file.write_at(text.as_bytes(), 0)?;
        file.sync()?;
        std::fs::File::open(path)?.sync_all()?;
        // The handle's directory entry is as durable as the fork.
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
        std::fs::File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(())
    };
    // Leave no handle half made behind, nor a branch without a handle.
    if let Err(err) = store.fork(child) {
        let _ = std::fs::remove_dir(path);
        return Err(err);
    }
    handled().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(path);
        let _ = store.lineage().delete(child);
    })
}

/// Deletes the store at `path`, which no store of this process has open: a fork's, whose handle
/// `path` is ([`Lineage::delete`]), whose extents go once no other branch holds them, and its
/// handle; or the store whose directory `path` is, with its files, once no fork of it is left.
/// Removing a store's directory otherwise, while forks of it live, loses their data.
pub fn delete_host(path: &Path, options: Options) -> Result<(), Error> {
    let (store, branch) = resolve(path)?;
    let lineage = lineage(&store, false, options)?;
    match branch {
        Some(branch) => {
            lineage.delete(branch)?;
            std::fs::remove_file(path.join(DESCRIPTION))?;
            std::fs::remove_dir(path)?;
        }
        None => {
            let forks: Vec<String> = (lineage.branches().into_iter())
                .filter(|(branch, _)| *branch != lineage.first())
                .map(|(branch, _)| format!("{branch:#x}"))
                .collect();
            ensure!(
                forks.is_empty(),
                "{}: forks of the store live: {}",
                path.display(),
                forks.join(", ")
            );
            // Not open in this process: no other opening of its first branch.
            drop(lineage.open_branch(lineage.first())?);
            OPEN.lock().unwrap().retain(|(open, _)| *open != store);
            drop(lineage);
            std::fs::remove_dir_all(&store)
                .with_context(|| format!("removing {}", store.display()))?;
        }
    }
    Ok(())
}

/// Whether the directory at `path` is a fork's handle.
pub fn is_handle(path: &Path) -> bool {
    std::fs::File::open(path.join(DESCRIPTION))
        .ok()
        .and_then(|file| {
            let mut first = vec![0; HANDLE.len()];
            std::io::Read::read_exact(&mut &file, &mut first).ok()?;
            Some(first == HANDLE.as_bytes())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_device::ExtentId;

    const BLOCK: usize = 4096;

    #[test]
    fn forks_open_through_their_handles() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = (temp.path().join("store"), temp.path().join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        store.write(ExtentId::new(3, 0), 0, &[7; BLOCK]).unwrap();
        fork_host(&store, &fork, 4).unwrap();
        assert!(is_handle(&fork) && !is_handle(&path));
        // The same store, shared, while the parent is open.
        let child = open_host(&fork, true, Options::default()).unwrap();
        assert!(Arc::ptr_eq(child.lineage(), store.lineage()));
        assert_eq!(child.description().branch, 4);
        let mut data = vec![0; BLOCK];
        child.read(ExtentId::new(3, 0), 0, &mut data).unwrap();
        assert_eq!(data, [7; BLOCK]);
        assert!(open_host(&fork, true, Options::default()).is_err(), "opened a branch twice");
        assert!(delete_host(&fork, Options::default()).is_err(), "deleted an open branch");
        drop((child, store));

        // And after the process let the store go.
        let child = open_host(&fork, true, Options::default()).unwrap();
        child.read(ExtentId::new(3, 0), 0, &mut data).unwrap();
        assert_eq!(data, [7; BLOCK]);
        drop(child);
        let error = delete_host(&path, Options::default()).expect_err("deleted a forked store");
        assert!(error.to_string().contains("forks of the store live: 0x4"), "{error}");
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
        let store = open_host(&path, true, Options::default()).unwrap();
        assert_eq!(store.lineage().branches(), vec![(3, None)]);
        assert!(delete_host(&path, Options::default()).is_err(), "deleted an open store");
        drop(store);
        delete_host(&path, Options::default()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn failed_forks_leave_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store");
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        // Its handle's place is taken.
        let taken = temp.path().join("taken");
        std::fs::create_dir(&taken).unwrap();
        assert!(fork_host(&store, &taken, 4).is_err());
        assert!(taken.exists());
        // Its branch is taken.
        assert!(store.lineage().open_branch(3).is_err(), "opened an open branch");
        let fork = temp.path().join("fork");
        fork_host(&store, &fork, 4).unwrap();
        assert!(fork_host(&store, &temp.path().join("again"), 4).is_err());
        assert!(!temp.path().join("again").exists());
        assert_eq!(store.lineage().branches(), vec![(3, None), (4, Some(3))]);
    }
}
