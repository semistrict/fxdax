// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stores in directories of the host's, as fxdax's images and extentd's stores are: a store's own
//! directory opens its first branch; a fork's directory is a handle, naming the store and the
//! branch, which opens that branch. A process shares a store between the branches of it it opens.

use crate::dir::{Dir, File as _, HostDir};
use crate::layout::{DESCRIPTION, Description, StoreFile};
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
    let (store, branch) = described(path)?;
    Ok((store.canonicalize()?, branch))
}

/// What the description at `path` says: the store's directory, `path` itself, or the one a handle
/// names, as it names it, whether it is there or not; and, a handle's, the branch of it to open.
fn described(path: &Path) -> Result<(PathBuf, Option<u64>), Error> {
    let text = std::fs::read_to_string(path.join(DESCRIPTION))
        .with_context(|| format!("reading {}'s description", path.display()))?;
    let mut lines = text.lines();
    if lines.next() != Some(HANDLE) {
        return Ok((path.to_owned(), None));
    }
    let fields: HashMap<&str, &str> = lines.filter_map(|line| line.split_once(' ')).collect();
    let store = fields.get("store").ok_or_else(|| anyhow!("{}: no store", path.display()))?;
    let branch = fields.get("branch").ok_or_else(|| anyhow!("{}: no branch", path.display()))?;
    let branch = branch.parse().with_context(|| format!("{}: bad branch", path.display()))?;
    Ok((PathBuf::from(store), Some(branch)))
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
/// directory at `path`, which opening `path` opens: returns the epoch of the fork's commit. The
/// store's directory must stay where it is for as long as the fork is.
pub fn fork_host(store: &Store<HostDir>, path: &Path, child: u64) -> Result<u64, Error> {
    std::fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    // The handle first, then the fork: a host crashing between them leaves a handle naming a
    // branch the store has not, which deleting it removes, never a branch no handle names, which
    // would keep what it inherited for ever.
    let forked = handle(store, path, child).and_then(|()| store.fork(child));
    // Leave no handle of a fork not made behind.
    forked.inspect_err(|_| {
        let _ = std::fs::remove_dir_all(path);
    })
}

/// The handle's description, before it is whole and in place.
fn new_description() -> String {
    format!("{DESCRIPTION}.new")
}

/// Writes the handle of `store`'s branch `child` into the directory `path`, durably, whole or not
/// at all: its description to a new file, synced, then renamed into place, the directories synced.
fn handle(store: &Store<HostDir>, path: &Path, child: u64) -> Result<(), Error> {
    let text = format!(
        "{HANDLE}\nstore {}\nbranch {child}\n",
        store.dir().path().canonicalize()?.display()
    );
    let file = HostDir::new(path, false)?.create(&new_description(), false)?;
    file.write_at(text.as_bytes(), 0)?;
    file.sync()?;
    std::fs::rename(path.join(new_description()), path.join(DESCRIPTION))?;
    std::fs::File::open(path)?.sync_all()?;
    // The handle's directory entry is as durable as the fork.
    std::fs::File::open(parent(path))?.sync_all()?;
    Ok(())
}

/// The directory the file or directory at `path` is in.
fn parent(path: &Path) -> &Path {
    path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// Deletes the store at `path`, which no store of this process has open: a fork's, whose handle
/// `path` is, and its handle; or the store whose directory `path` is ([`Lineage::delete`]). Its
/// extents go once no other branch holds them: a store whose forks live keeps its directory, which
/// holds what they share of it, and which no longer opens; the directory goes with the last of its
/// branches (TASK-39.4). A deletion that stopped partway is finished by deleting again. Removing a
/// store's directory otherwise, while forks of it live, loses their data. It fails while another
/// process has branches of the store open.
pub fn delete_host(path: &Path, options: Options) -> Result<(), Error> {
    delete_host_with(path, options, |_, _| Ok(()), |_, _| Ok(()))
}

/// Deletes the store at `path` as [`delete_host`] does, and what else keeps its branch, by
/// calling `before` and `after` with the store's directory and the branch, with the directory
/// taken for this process meanwhile ([`Lineage::holding`]). `before` goes once the branch is found
/// open nowhere, before the store deletes it: to delete what keeps it elsewhere, as a bucket does.
/// `after` goes once the store has deleted it, before the handle naming it goes: to remove what
/// else of it the store's directory holds, as the records of its uploads. A store of this process
/// that opens the branch as `before` goes keeps it here: the deletion fails then, `before` done,
/// and deleting again, once the branch is closed, finishes it. A deletion that stopped
/// partway, deleted again, calls `before` again unless the branch went, and `after` unless the
/// handle did, so that each sees its work done (TASK-79.71). The store's directory goes with its
/// last branch before the handle naming it, so that a deletion that stopped in between finds it by
/// that handle (TASK-79.71): a handle naming a store whose directory went, or was going, names a
/// branch that went with it. A fork's handle that names no branch goes, calling neither: one whose
/// deletion stopped once its description was gone, or whose fork stopped before its description
/// was in place.
pub fn delete_host_with(
    path: &Path,
    options: Options,
    before: impl FnOnce(&Path, u64) -> Result<(), Error>,
    after: impl FnOnce(&Path, u64) -> Result<(), Error>,
) -> Result<(), Error> {
    if !path.join(DESCRIPTION).exists() {
        // A store's directory whose removal stopped partway, its branches all gone.
        if path.join(deleted_description()).exists() {
            return remove_store(path);
        }
        // A fork's handle that names no branch is empty but for its description not in place, if
        // any.
        remove(removals::remove_file(&path.join(new_description())))?;
        if std::fs::read_dir(path)?.next().is_none() {
            removals::remove_dir(path)?;
            return Ok(());
        }
    }
    let (store, branch) = described(path)?;
    // A fork's handle naming a store whose directory went, or was going, with its last branch.
    if branch.is_some() && !store.join(DESCRIPTION).exists() {
        remove_store(&store)?;
        return remove_handle(path);
    }
    let store = store.canonicalize()?;
    let lineage = lineage(&store, false, options)?;
    let target = branch.unwrap_or_else(|| lineage.first());
    lineage.holding(|| {
        // A branch deleted already, by a deletion that stopped partway, is done but for what else
        // of it the store's directory holds, the directory, if it was the last, and its handle.
        if lineage.has(target) {
            ensure!(!lineage.is_open(target), "branch {target:#x} is open");
            before(&store, target)?;
            lineage.delete(target)?;
        }
        after(&store, target)
    })?;
    if lineage.branches().is_empty() {
        OPEN.lock().unwrap().retain(|(open, _)| *open != store);
        drop(lineage);
        remove_store(&store)?;
    }
    if branch.is_some() {
        remove_handle(path)?;
    }
    Ok(())
}

/// The name a store's description goes by as its directory goes ([`remove_store`]).
fn deleted_description() -> String {
    format!("{DESCRIPTION}.deleted")
}

/// Removes the directory of the store at `store`, whose branches all went, unless it went already:
/// whole, or so that removing it again finishes it. Its description goes first, renamed to
/// [`deleted_description`], durably, so that the directory is no store from then on, but one
/// going, which a deletion resumed finds so; then the rest, then that, then the directory. A
/// directory neither a store nor going is removed only if it is empty. Then the directory it was
/// in is synced, so that it is gone durably before the handle naming it goes: a host losing power,
/// its file system keeping the handle's removal and not the directory's, would leave the directory
/// with nothing naming it (TASK-79.77).
fn remove_store(store: &Path) -> Result<(), Error> {
    let name = deleted_description();
    let deleted = store.join(&name);
    let removing = || -> std::io::Result<()> {
        match removals::rename(&store.join(DESCRIPTION), &deleted) {
            Ok(()) => removals::sync_dir(store)?,
            // Going already, or gone: another deletion of its last branches, or one that stopped,
            // renamed it first.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        if deleted.exists() {
            // None if gone since, another deletion of its last branches having removed it: this
            // one still syncs the directory it was in before its handle goes.
            let entries = match std::fs::read_dir(store) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                entries => Some(entries?),
            };
            for entry in entries.into_iter().flatten() {
                let entry = entry?;
                if entry.file_name() == *name {
                    continue;
                }
                remove(match entry.file_type()?.is_dir() {
                    true => removals::remove_dir_all(&entry.path()),
                    false => removals::remove_file(&entry.path()),
                })?;
            }
            remove(removals::remove_file(&deleted))?;
        }
        remove(removals::remove_dir(store))?;
        // Gone durably, the directory it was in synced.
        removals::sync_dir(parent(store))
    };
    removing().with_context(|| format!("removing {}", store.display()))
}

/// Removes the fork's handle at `path`, unless it went already: its description, then its
/// directory.
fn remove_handle(path: &Path) -> Result<(), Error> {
    remove(removals::remove_file(&path.join(DESCRIPTION)))?;
    Ok(remove(removals::remove_dir(path))?)
}

/// The changes a deletion makes to the host's file system as it removes a store's directory or a
/// fork's handle ([`remove_store`], [`remove_handle`]): std's, each a step, which a test records,
/// and stops after a number of, as a crash would stop them (TASK-79.77).
mod removals {
    use std::io;
    use std::path::Path;

    /// What a step changes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Change {
        /// A file renamed to the path.
        Renamed,
        /// The file or directory at the path removed.
        Removed,
        /// The directory at the path's entries made durable.
        Synced,
    }

    /// Renames the file at `from` to `to`.
    pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
        step(Change::Renamed, to)?;
        std::fs::rename(from, to)
    }

    /// Removes the file at `path`.
    pub(super) fn remove_file(path: &Path) -> io::Result<()> {
        step(Change::Removed, path)?;
        std::fs::remove_file(path)
    }

    /// Removes the empty directory at `path`.
    pub(super) fn remove_dir(path: &Path) -> io::Result<()> {
        step(Change::Removed, path)?;
        std::fs::remove_dir(path)
    }

    /// Removes the directory at `path`, and all it holds.
    pub(super) fn remove_dir_all(path: &Path) -> io::Result<()> {
        step(Change::Removed, path)?;
        std::fs::remove_dir_all(path)
    }

    /// Makes the entries of the directory at `path` durable.
    pub(super) fn sync_dir(path: &Path) -> io::Result<()> {
        step(Change::Synced, path)?;
        std::fs::File::open(path)?.sync_all()
    }

    /// Goes on to the step that makes `change` at `path`: at once, but in a test.
    #[cfg(not(test))]
    fn step(_: Change, _: &Path) -> io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    use super::tests::step;
}

/// What removing something gave, but for its being gone already.
fn remove(removed: std::io::Result<()>) -> std::io::Result<()> {
    match removed {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// The directory of the store at `path`, and the branch of it opening `path` opens
/// ([`open_host`]): its first branch, or, if `path` is a fork's handle, that fork's.
pub fn resolve_host(path: &Path) -> Result<(PathBuf, u64), Error> {
    let (store, branch) = resolve(path)?;
    let branch = match branch {
        Some(branch) => branch,
        None => {
            let text = std::fs::read_to_string(store.join(DESCRIPTION))
                .with_context(|| format!("reading {}'s description", store.display()))?;
            StoreFile::parse(&text)?.first
        }
    };
    Ok((store, branch))
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
    use removals::Change;
    use std::cell::RefCell;
    use storage_device::ExtentId;

    const BLOCK: usize = 4096;

    /// The steps of this thread's deletions' removals ([`removals`]): those taken, and how many
    /// more may be, if limited.
    #[derive(Default)]
    struct Steps {
        taken: Vec<(Change, PathBuf)>,
        left: Option<usize>,
    }

    thread_local! {
        static STEPS: RefCell<Steps> = RefCell::default();
    }

    /// Takes the step that makes `change` at `path`, recording it; or fails, as a crash would stop
    /// it, once the steps allowed are taken ([`stop_after`]).
    pub(super) fn step(change: Change, path: &Path) -> std::io::Result<()> {
        STEPS.with_borrow_mut(|steps| {
            match &mut steps.left {
                Some(0) => return Err(std::io::Error::other("stopped")),
                Some(left) => *left -= 1,
                None => {}
            }
            steps.taken.push((change, path.to_owned()));
            Ok(())
        })
    }

    /// Stops this thread's deletions' removals after `steps` more steps, or never if `None`.
    fn stop_after(steps: Option<usize>) {
        STEPS.with_borrow_mut(|taken| taken.left = steps);
    }

    /// The steps this thread's deletions' removals took since this was last asked.
    fn taken() -> Vec<(Change, PathBuf)> {
        STEPS.with_borrow_mut(|steps| std::mem::take(&mut steps.taken))
    }

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
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
        let store = open_host(&path, true, Options::default()).unwrap();
        assert_eq!(store.lineage().branches(), vec![(3, None)]);
        assert!(delete_host(&path, Options::default()).is_err(), "deleted an open store");
        drop(store);
        delete_host(&path, Options::default()).unwrap();
        assert!(!path.exists());
    }

    /// A deletion that stopped partway, its branch deleted but its handle or directory not, is
    /// finished by deleting again.
    #[test]
    fn stopped_deletions_finish() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = (temp.path().join("store"), temp.path().join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        fork_host(&store, &fork, 4).unwrap();
        let lineage = store.lineage().clone();
        drop(store);
        // Stopped after the records said so.
        lineage.delete(4).unwrap();
        lineage.delete(3).unwrap();
        drop(lineage);
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
        assert!(!path.exists(), "the last branch gone, its directory goes");

        // A handle whose description went, and nothing else.
        std::fs::create_dir(&fork).unwrap();
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
    }

    /// A store in the directory `store` of `temp`, deleted, and its fork, whose handle is `fork`
    /// there, living on as its last branch: the store's directory holds what the fork shares of
    /// it. Returns the two paths.
    fn last_fork(temp: &Path) -> (PathBuf, PathBuf) {
        let (path, fork) = (temp.join("store"), temp.join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        store.write(ExtentId::new(3, 0), 0, &[7; BLOCK]).unwrap();
        fork_host(&store, &fork, 4).unwrap();
        drop(store);
        delete_host(&path, Options::default()).unwrap();
        assert!(path.exists());
        (path, fork)
    }

    /// What a deletion that stopped as it removed the directory of the store at `path`, its
    /// branches all gone, left of it: its description renamed, which goes first, and part of the
    /// rest gone, here its branches' records.
    fn stopped_removing(path: &Path) {
        std::fs::rename(path.join(DESCRIPTION), path.join("store.deleted")).unwrap();
        std::fs::remove_dir_all(path.join(crate::layout::BRANCHES)).unwrap();
    }

    /// TASK-79.71: deleting a store's last branch by its fork's handle removes the store's
    /// directory before the handle, so that a deletion that stopped once the handle's description
    /// went is finished by deleting again: no handle names the store then. Here the handle holds a
    /// file of no store's, which removing its directory fails on, until it goes, as a crash left
    /// the handle.
    #[test]
    fn deletions_of_last_branches_stopped_at_their_handles_finish() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = last_fork(temp.path());
        let stray = fork.join("stray");
        std::fs::write(&stray, "").unwrap();
        let stopped = delete_host(&fork, Options::default()).unwrap_err();
        let kind = stopped.downcast_ref::<std::io::Error>().map(std::io::Error::kind);
        assert_eq!(kind, Some(std::io::ErrorKind::DirectoryNotEmpty));
        assert_eq!((fork.join(DESCRIPTION).exists(), path.exists()), (false, false));
        std::fs::remove_file(&stray).unwrap();
        delete_host(&fork, Options::default()).unwrap();
        assert_eq!((fork.exists(), path.exists()), (false, false));
    }

    /// TASK-79.71: so is one that stopped as the store's directory went, by the fork's handle,
    /// which names it still, or by the store's directory, its last branch its first.
    #[test]
    fn deletions_stopped_as_their_stores_directories_went_finish() {
        let stopping = |_: &Path, _| Err(anyhow!("stopped"));
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = last_fork(temp.path());
        delete_host_with(&fork, Options::default(), |_, _| Ok(()), stopping).unwrap_err();
        stopped_removing(&path);
        delete_host(&fork, Options::default()).unwrap();
        assert_eq!((fork.exists(), path.exists()), (false, false));

        let path = temp.path().join("alone");
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 5 };
        drop(create_host(&path, description, true, Options::default()).unwrap());
        delete_host_with(&path, Options::default(), |_, _| Ok(()), stopping).unwrap_err();
        stopped_removing(&path);
        delete_host(&path, Options::default()).unwrap();
        assert!(!path.exists());
    }

    /// TASK-79.77: a deletion of a store's last branch by its fork's handle that stopped at any
    /// step is finished by deleting again, which leaves nothing of the store, nor of the handle:
    /// one stopped before the store deleted the branch, or after; and one stopped before each step
    /// of the removals that follow, of the store's directory, then of the handle ([`removals`]),
    /// as a crash would stop them.
    #[test]
    fn deletions_of_last_branches_stopped_at_any_step_finish() {
        let finished = |temp: &Path, fork: &Path, stopped: &str| {
            delete_host(fork, Options::default())
                .unwrap_or_else(|err| panic!("stopped {stopped}: {err:#}"));
            let left: Vec<_> = std::fs::read_dir(temp).unwrap().collect();
            assert_eq!(left.len(), 0, "stopped {stopped}: {left:?} left");
        };
        let stopping = |_: &Path, _| Err(anyhow!("stopped"));
        let temp = tempfile::tempdir().unwrap();
        let (_, fork) = last_fork(temp.path());
        delete_host_with(&fork, Options::default(), stopping, |_, _| Ok(())).unwrap_err();
        finished(temp.path(), &fork, "before the branch went");
        let temp = tempfile::tempdir().unwrap();
        let (_, fork) = last_fork(temp.path());
        delete_host_with(&fork, Options::default(), |_, _| Ok(()), stopping).unwrap_err();
        finished(temp.path(), &fork, "after the branch went");
        // The removals' steps, as a deletion stopped at none takes them.
        let temp = tempfile::tempdir().unwrap();
        let (_, fork) = last_fork(temp.path());
        taken();
        delete_host(&fork, Options::default()).unwrap();
        for (stop, (change, path)) in taken().into_iter().enumerate() {
            let at = format!("before step {stop}, {change:?} {:?}", path.file_name().unwrap());
            let temp = tempfile::tempdir().unwrap();
            let (_, fork) = last_fork(temp.path());
            stop_after(Some(stop));
            let stopped = delete_host(&fork, Options::default());
            stop_after(None);
            stopped.expect_err(&format!("went on {at}"));
            finished(temp.path(), &fork, &at);
        }
    }

    /// TASK-79.77: a deletion of a store's last branch by its fork's handle removes the store's
    /// directory durably, the directory it was in synced, before the handle's description goes: a
    /// host losing power, its file system keeping the handle's removal and not the directory's,
    /// would leave the store's directory with nothing naming it.
    #[test]
    fn deletions_of_last_branches_remove_the_stores_directory_durably_before_the_handle() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = last_fork(temp.path());
        let store = path.canonicalize().unwrap();
        let parent = store.parent().unwrap().to_owned();
        // None but the deletion's.
        taken();
        delete_host(&fork, Options::default()).unwrap();
        // Those of the store's directory itself, of the directory it was in, and of the handle.
        let steps: Vec<_> = taken()
            .into_iter()
            .filter(|(_, at)| *at == store || *at == parent || at.starts_with(&fork))
            .collect();
        assert_eq!(
            steps,
            [
                (Change::Synced, store.clone()),
                (Change::Removed, store),
                (Change::Synced, parent),
                (Change::Removed, fork.join(DESCRIPTION)),
                (Change::Removed, fork),
            ]
        );
    }

    /// A store deleted while a fork of it lives keeps its directory, which holds what the fork
    /// shares, until the fork goes too (TASK-39.4).
    #[test]
    fn stores_go_once_their_forks_do() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = (temp.path().join("store"), temp.path().join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        store.write(ExtentId::new(3, 0), 0, &[7; BLOCK]).unwrap();
        fork_host(&store, &fork, 4).unwrap();
        drop(store);

        delete_host(&path, Options::default()).unwrap();
        assert!(path.exists());
        let error = open_host(&path, true, Options::default()).err().expect("opened");
        assert_eq!(
            error.to_string(),
            "the store was deleted: its directory holds its forks' data alone"
        );
        let child = open_host(&fork, true, Options::default()).unwrap();
        let mut data = vec![0; BLOCK];
        child.read(ExtentId::new(3, 0), 0, &mut data).unwrap();
        assert_eq!(data, [7; BLOCK]);
        child.write(ExtentId::new(4, 0), 0, &[9; BLOCK]).unwrap();
        child.commit(None).unwrap();
        drop(child);

        // And after the process let the store go.
        OPEN.lock().unwrap().clear();
        let child = open_host(&fork, true, Options::default()).unwrap();
        child.read(ExtentId::new(4, 0), 0, &mut data).unwrap();
        assert_eq!(data, [9; BLOCK]);
        drop(child);
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
        assert!(!path.exists());
    }

    /// TASK-79.20: a fork's handle is whole and durable before its branch is recorded: a host
    /// crashing between the two leaves a handle naming a branch the store has not, which deleting
    /// the handle removes ([`stopped_deletions_finish`]), never a branch no handle names, which
    /// would keep what it inherited for ever.
    #[test]
    fn forks_handles_come_before_their_branches() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = (temp.path().join("store"), temp.path().join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        // A write in progress, which the fork's commit waits for.
        let writing = store.begin_write(ExtentId::new(3, 0), 0, BLOCK as u64).unwrap();
        let (waited, handle, recorded) = std::thread::scope(|scope| {
            let forking = scope.spawn(|| fork_host(&store, &fork, 4));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !store.holding_writes() && std::time::Instant::now() < deadline {
                std::thread::yield_now();
            }
            // The host crashing now, what is there.
            let seen = (
                store.holding_writes(),
                std::fs::read_to_string(fork.join(DESCRIPTION)).ok(),
                store.lineage().has(4),
            );
            drop(writing);
            forking.join().unwrap().unwrap();
            seen
        });
        let named =
            format!("{HANDLE}\nstore {}\nbranch 4\n", path.canonicalize().unwrap().display());
        assert_eq!((waited, handle, recorded), (true, Some(named), false));
        assert!(store.lineage().has(4));
        assert_eq!(std::fs::read_dir(&fork).unwrap().count(), 1, "only its description");
    }

    /// TASK-79.71: deleting a fork calls `before` while the store has its branch, and `after` once
    /// it has not, each with the store's directory and the branch, both before the fork's handle
    /// goes. A deletion that `after` stopped keeps the handle, and, deleted again, calls `after`
    /// alone, then removes it.
    #[test]
    fn deletions_call_what_deletes_the_rest_of_a_branch_before_and_after_the_store_does() {
        let temp = tempfile::tempdir().unwrap();
        let (path, fork) = (temp.path().join("store"), temp.path().join("fork"));
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, true, Options::default()).unwrap();
        fork_host(&store, &fork, 4).unwrap();
        let lineage = store.lineage().clone();
        // Each call: which, with what, whether the store had the branch, and whether the handle
        // was there.
        let calls = std::cell::RefCell::new(Vec::new());
        let call = |name: &'static str| {
            let (calls, lineage, fork) = (&calls, &lineage, &fork);
            move |dir: &Path, branch: u64| -> Result<(), Error> {
                let (had, handle) = (lineage.has(branch), fork.join(DESCRIPTION).exists());
                calls.borrow_mut().push((name, dir.to_owned(), branch, had, handle));
                Ok(())
            }
        };
        let stopping = |dir: &Path, branch| call("after")(dir, branch).and(Err(anyhow!("stopped")));
        let stopped = delete_host_with(&fork, Options::default(), call("before"), stopping);
        assert_eq!(stopped.map_err(|err| err.to_string()), Err("stopped".to_owned()));
        delete_host_with(&fork, Options::default(), call("before"), call("after")).unwrap();
        let dir = path.canonicalize().unwrap();
        assert_eq!(
            calls.into_inner(),
            [
                ("before", dir.clone(), 4, true, true),
                ("after", dir.clone(), 4, false, true),
                ("after", dir, 4, false, true)
            ]
        );
        assert!(!fork.exists());
    }

    /// TASK-79.20: a fork's handle whose description was never put in place, its host crashing
    /// while it wrote it, is deleted.
    #[test]
    fn handles_never_described_are_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let fork = temp.path().join("fork");
        std::fs::create_dir(&fork).unwrap();
        std::fs::write(fork.join(format!("{DESCRIPTION}.new")), HANDLE).unwrap();
        delete_host(&fork, Options::default()).unwrap();
        assert!(!fork.exists());
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
