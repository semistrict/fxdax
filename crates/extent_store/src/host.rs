// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stores in directories of the host's, as fxdax's images and extentd's stores are: a directory
//! holds one store, of one branch (TASK-165.7). Forks, points and deletions are the bucket's,
//! which the store's description names: the store makes nothing of them but the commit that names
//! a fork ([`Store::commit_fork`]).

use crate::dir::{Dir as _, HostDir};
use crate::layout::{DESCRIPTION, Description, StoreFile};
use crate::store::{Options, Store};
use anyhow::{Context as _, Error};
use std::path::Path;

/// Opens the store at `path`.
pub fn open_host(path: &Path, direct: bool, options: Options) -> Result<Store<HostDir>, Error> {
    Store::open(HostDir::new(path, direct)?, options)
}

/// Makes a new store at `path`, which must be a directory holding no store, as `description`
/// describes, and opens it. fxdax: its description names `bucket`, the URL of the bucket its
/// branch is kept in, as every host's store's does (TASK-165.6). Its description is made last:
/// until it is durable, the directory is no store, whatever else is in it.
pub fn create_host(
    path: &Path,
    description: Description,
    bucket: &str,
    direct: bool,
    options: Options,
) -> Result<Store<HostDir>, Error> {
    Store::create_kept(HostDir::new(path, direct)?, description, Some(bucket), options)
}

/// Deletes the store at `path`, which no store has open: `before` goes once that is found so, with
/// the store's branch, before its directory goes, to delete what keeps the branch elsewhere, as a
/// bucket does; then the directory, whole. A deletion that stopped partway is finished by deleting
/// again, which calls `before` again unless the directory's removal had begun. A directory with no
/// description is no store, but what a store's making left, or its removal: it goes, `before`
/// called for nothing. It fails while another process has the store open, or this one.
pub fn delete_host(
    path: &Path,
    before: impl FnOnce(u64) -> Result<(), Error>,
) -> Result<(), Error> {
    if !path.join(DESCRIPTION).exists() {
        return remove_store(path);
    }
    let branch = host_branch(path)?;
    let dir = HostDir::new(path, false)?;
    dir.take().context("taking the store's directory")?;
    let removed = dir.claim().context("taking the store's directory").and_then(|()| {
        let removed = before(branch).and_then(|()| {
            failpoint!("delete::branch");
            remove_store(path)
        });
        dir.unclaim();
        removed
    });
    dir.let_go();
    removed
}

/// The name a store's description goes by as its directory goes ([`remove_store`]).
fn deleted_description() -> String {
    format!("{DESCRIPTION}.deleted")
}

/// Removes the directory of the store at `store`, unless it went already: whole, or so that
/// removing it again finishes it. Its description goes first, renamed to [`deleted_description`],
/// durably, so that the directory is no store from then on, but one going, which a deletion resumed
/// finds so; then the rest, then that, then the directory. Then the directory it was in is synced,
/// so that it is gone durably (TASK-79.77).
fn remove_store(store: &Path) -> Result<(), Error> {
    let name = deleted_description();
    let deleted = store.join(&name);
    let removing = || -> std::io::Result<()> {
        match removals::rename(&store.join(DESCRIPTION), &deleted) {
            Ok(()) => removals::sync_dir(store)?,
            // Going already, or no store: a deletion that stopped, or a making that did.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
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
        remove(removals::remove_dir(store))?;
        // Gone durably, the directory it was in synced.
        removals::sync_dir(parent(store))
    };
    removing().with_context(|| format!("removing {}", store.display()))
}

/// The directory `path` is in.
fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// The changes a deletion makes to the host's file system as it removes a store's directory
/// ([`remove_store`]): std's, each a step, which a test records, and stops after a number of, as a
/// crash would stop them (TASK-79.77).
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

/// The branch of the store at `path`, as its description names it.
pub fn host_branch(path: &Path) -> Result<u64, Error> {
    Ok(store_file(path)?.branch)
}

/// fxdax: the URL of the bucket the store at `path` keeps its branch in, as its description names
/// it: none for a store of memory's, which names none (TASK-165.6).
pub fn host_bucket(path: &Path) -> Result<Option<String>, Error> {
    Ok(store_file(path)?.bucket)
}

/// The description of the store whose directory is `store`.
fn store_file(store: &Path) -> Result<StoreFile, Error> {
    let text = std::fs::read_to_string(store.join(DESCRIPTION))
        .with_context(|| format!("reading {}'s description", store.display()))?;
    StoreFile::parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use removals::Change;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use storage_device::ExtentId;

    const BLOCK: usize = 4096;

    /// The bucket the tests' stores name, which nothing here reaches.
    const BUCKET: &str = "memory:///host";

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

    /// Stops this thread's deletions' removals after `left` more steps, or never if `None`.
    fn stop_after(left: Option<usize>) {
        STEPS.with_borrow_mut(|steps| steps.left = left);
    }

    /// The steps this thread's deletions' removals took since this was last asked.
    fn taken() -> Vec<(Change, PathBuf)> {
        STEPS.with_borrow_mut(|steps| std::mem::take(&mut steps.taken))
    }

    /// A store at `store` in `temp`, holding an extent: its path.
    fn stored(temp: &Path) -> PathBuf {
        let path = temp.join("store");
        std::fs::create_dir(&path).unwrap();
        let description = Description { block_size: BLOCK as u32, capacity: 1 << 20, branch: 3 };
        let store = create_host(&path, description, BUCKET, true, Options::default()).unwrap();
        store.write(ExtentId::new(3, 0), 0, &[7; BLOCK]).unwrap();
        store.commit(None).unwrap();
        path
    }

    /// A deletion that stopped partway, its description renamed and part of the rest gone, is
    /// finished by deleting again, which calls nothing to delete the branch elsewhere; and so is
    /// a directory a store's making left before its description, holding files of no store's.
    #[test]
    fn stopped_deletions_finish() {
        let temp = tempfile::tempdir().unwrap();
        let path = stored(temp.path());
        std::fs::rename(path.join(DESCRIPTION), path.join("store.deleted")).unwrap();
        std::fs::remove_dir_all(path.join(crate::layout::INDEX)).unwrap();
        delete_host(&path, |_| Err(anyhow!("called"))).unwrap();
        assert!(!path.exists());

        let unmade = temp.path().join("unmade");
        std::fs::create_dir_all(unmade.join("segments")).unwrap();
        std::fs::write(unmade.join("stray"), "").unwrap();
        delete_host(&unmade, |_| Err(anyhow!("called"))).unwrap();
        assert!(!unmade.exists());
    }

    /// A deletion calls `before` with the store's branch, while the store is open nowhere, before
    /// its directory goes: one `before` stopped leaves the store, which deleting again finishes.
    /// One of a store open in this process fails, calling nothing.
    #[test]
    fn deletions_delete_their_branch_elsewhere_first() {
        let temp = tempfile::tempdir().unwrap();
        let path = stored(temp.path());
        let open = open_host(&path, true, Options::default()).unwrap();
        let refused = delete_host(&path, |_| Err(anyhow!("called")));
        assert!(refused.is_err() && path.join(DESCRIPTION).exists(), "{refused:?}");
        drop(open);
        let stopped = delete_host(&path, |branch| Err(anyhow!("stopped at {branch}")));
        assert_eq!(stopped.map_err(|err| err.to_string()), Err("stopped at 3".to_owned()));
        assert!(
            open_host(&path, true, Options::default()).is_ok(),
            "a store stopped opens no more"
        );
        let called = std::cell::Cell::new(None);
        delete_host(&path, |branch| Ok(called.set(Some(branch)))).unwrap();
        assert_eq!((called.get(), path.exists()), (Some(3), false));
    }

    /// TASK-79.77: a deletion stopped at any step is finished by deleting again, which leaves
    /// nothing of the store: one stopped before the branch went elsewhere, and one stopped before
    /// each step of the removals that follow ([`removals`]), as a crash would stop them.
    #[test]
    fn deletions_stopped_at_any_step_finish() {
        // Stopped where it was stopped, and not before, by anything else.
        let stopped = |deleted: Result<(), Error>, at: &str| {
            let err = deleted.expect_err(&format!("went on {at}"));
            assert_eq!(err.root_cause().to_string(), "stopped", "stopped {at}: {err:#}");
        };
        let finished = |temp: &Path, path: &Path, at: &str| {
            delete_host(path, |_| Ok(())).unwrap_or_else(|err| panic!("stopped {at}: {err:#}"));
            let left: Vec<_> = std::fs::read_dir(temp).unwrap().collect();
            assert_eq!(left.len(), 0, "stopped {at}: {left:?} left");
        };
        let at = "before the branch went";
        let temp = tempfile::tempdir().unwrap();
        let path = stored(temp.path());
        stopped(delete_host(&path, |_| Err(anyhow!("stopped"))), at);
        finished(temp.path(), &path, at);
        // The removals' steps, as a deletion stopped at none takes them.
        let temp = tempfile::tempdir().unwrap();
        let path = stored(temp.path());
        taken();
        delete_host(&path, |_| Ok(())).unwrap();
        for (stop, (change, at)) in taken().into_iter().enumerate() {
            let at = format!("before step {stop}, {change:?} {:?}", at.file_name().unwrap());
            let temp = tempfile::tempdir().unwrap();
            let path = stored(temp.path());
            stop_after(Some(stop));
            let deleted = delete_host(&path, |_| Ok(()));
            stop_after(None);
            stopped(deleted, &at);
            finished(temp.path(), &path, &at);
        }
    }

    /// TASK-79.77: a deletion renames the store's description first, durably, so that the
    /// directory is no store from then on, and removes the directory durably, the directory it
    /// was in synced, last.
    #[test]
    fn deletions_remove_the_stores_directory_durably() {
        let temp = tempfile::tempdir().unwrap();
        let path = stored(temp.path());
        let parent = super::parent(&path).to_owned();
        taken();
        delete_host(&path, |_| Ok(())).unwrap();
        let steps = taken();
        let removed = |at: PathBuf| {
            steps.iter().position(|(change, step)| *change == Change::Removed && *step == at)
        };
        assert_eq!(steps[0], (Change::Renamed, path.join("store.deleted")));
        assert_eq!(steps[1], (Change::Synced, path.clone()));
        assert_eq!(
            &steps[steps.len() - 3..],
            [
                (Change::Removed, path.join("store.deleted")),
                (Change::Removed, path.clone()),
                (Change::Synced, parent.clone()),
            ]
        );
        let (segments, description) = (path.join("segments"), path.join("store.deleted"));
        assert!(removed(segments).unwrap() < removed(description).unwrap());
    }
}
