// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extent stores in the host's directories, as both of fxdax's host stacks keep them: the images
//! [`Storage`](crate::Storage) mounts, and the stores [`ExtentStore`](crate::ExtentStore) serves a
//! guest. Either makes, opens and deletes its stores here, so that both do it alike. Images also
//! clean theirs here after commits ([`clean_after_commit`]); the daemon's stores, behind their
//! guests' commits ([`clean_behind`]).
//!
//! Both open stores with the same options, as they must: the process shares a directory's store
//! between the branches of it that it opens, with the options of whichever opened it first
//! (`extent_store::host`). A fork needs none of its own: `extent_store::fork_host` makes its
//! handle, which [`open`] opens.

use crate::error::{Errno, Error};
use anyhow::Context as _;
use extent_store::{Description, HostDir, Options, Store};
use std::path::Path;

/// The size of the blocks stores' extents are read and written in. Their capacities are multiples
/// of it.
pub(crate) const BLOCK_SIZE: u32 = 4096;

/// Whether stores' files bypass the host's page cache, where the host's file system allows: the
/// in-process kernel caches their pages already.
const DIRECT: bool = true;

/// The bytes no extent is in, of a store's segments that cleaning takes, past which cleaning it is
/// due: or any, once they count as used, past the store's capacity's worth, as a store whose
/// capacity is short of this is refused room for them ([`Store::cleaning_due`], TASK-79.2).
const CLEAN_PAST: u64 = 64 << 20;

/// The fraction of a segment its extents must hold for cleaning to leave it be.
const CLEAN_BELOW: f64 = 0.5;

/// fxdax: the most bytes a pass of cleaning behind a store's commits copies ([`clean_behind`]):
/// what it holds its connection's going up for, at most.
const CLEAN_PASS: u64 = 64 << 20;

/// A new store's branch, or a fork's: random, so that stores made apart, and the forks of one,
/// mint extent IDs of their own (backlog doc-4).
pub(crate) fn new_branch() -> u64 {
    rand::random::<u64>().max(1)
}

/// The options stores are opened with: their extents hold at most what the guest's device says
/// one does (doc-4's `max_extent`).
fn options() -> Options {
    Options { max_extent: crate::ExtentStore::MAX_EXTENT, ..Options::default() }
}

/// Makes a new store at `path`, which must not exist, holding at most `capacity` bytes, of
/// `branch`, and opens it. Its directory entry is as durable as it is. A store that is not made
/// whole is removed; what is at `path` already is left as it is.
pub(crate) fn create(path: &Path, capacity: u64, branch: u64) -> anyhow::Result<Store<HostDir>> {
    std::fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    let made = || -> anyhow::Result<Store<HostDir>> {
        let description = Description { block_size: BLOCK_SIZE, capacity, branch };
        let store = extent_store::create_host(path, description, DIRECT, options())?;
        let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
        std::fs::File::open(parent.unwrap_or(Path::new(".")))?.sync_all()?;
        Ok(store)
    };
    made().inspect_err(|_| {
        // Leave nothing half made behind.
        let _ = std::fs::remove_dir_all(path);
    })
}

/// Opens the store at `path`, or the fork whose handle `path` is, as of its last commit.
pub(crate) fn open(path: &Path) -> anyhow::Result<Store<HostDir>> {
    extent_store::open_host(path, DIRECT, options())
}

/// Deletes the store at `path`, or the fork whose handle `path` is, which nothing has open
/// (`extent_store::delete_host`): its extents go but for those other branches hold, and a store
/// whose forks live keeps its directory, holding what they share of it, until the last of them
/// goes (TASK-39.4). With `bucket`, its branch there is deleted first, its forks' branches living
/// on; the records of its branch's placement and uploads go with it either way, once it went from
/// the store, before the handle naming it. A deletion that stopped partway is finished by deleting
/// again, which finds them by that handle (TASK-79.71), and a fork's handle whose fork stopped
/// before its description was in place goes (TASK-79.20).
///
/// Fails with `ENOENT` if nothing is at `path`, and with `ESTALE` if a later placement of the
/// store holds its branch in `bucket`.
pub(crate) fn delete(
    path: &Path,
    bucket: Option<&extent_remote::store::Store>,
) -> Result<(), Error> {
    // As this copy's placement, or this host's, deletes it: not from under a later one (`ESTALE`).
    delete_with(path, |dir, branch| match bucket {
        Some(bucket) => {
            let placed = crate::remote::placed_token(dir, branch)?;
            let token = placed.map_or_else(crate::remote::host_token, Ok)?;
            crate::remote::delete_branch(bucket, branch, token)
        }
        None => Ok(()),
    })
}

/// Deletes the store at `path`, or the fork whose handle `path` is, as [`delete`] does, `elsewhere`
/// deleting its branch where else it is kept, given the store's directory and the branch: once the
/// branch is found open nowhere here, before it goes here, so that a deletion that stopped partway,
/// the host crashing or `elsewhere` failing, leaves the branch named by its handle still, which
/// deleting again finishes, never kept elsewhere alone.
pub(crate) fn delete_with(
    path: &Path,
    elsewhere: impl FnOnce(&Path, u64) -> Result<(), crate::remote::Failure>,
) -> Result<(), Error> {
    if !path.exists() {
        return Err(Errno::ENOENT.into());
    }
    extent_store::delete_host_with(
        path,
        options(),
        |dir, branch| Ok(elsewhere(dir, branch)?),
        crate::remote::unmark,
    )
    .map_err(|err| {
        let stale = err.chain().any(|cause| {
            cause.downcast_ref::<crate::remote::Failure>() == Some(&crate::remote::Failure::Stale)
        });
        if stale { Error::new(err.context(Errno::ESTALE)) } else { Error::new(err) }
    })
}

/// Cleans `store`, just committed, if cleaning is due ([`cleaning_due`]): it rewrites the segments
/// whose extents hold less than `CLEAN_BELOW` of them, and commits.
///
/// Images clean so after each of their device's commits. The extent daemon's stores clean behind
/// their guests' commits ([`clean_behind`]), which this commit would stall.
pub(crate) fn clean_after_commit(store: &Store<HostDir>) -> anyhow::Result<()> {
    if cleaning_due(store)? {
        let cleaned = store.clean(CLEAN_BELOW)?;
        log::info!("fxdax: cleaned {}: {cleaned:?}", store.dir().path().display());
    }
    Ok(())
}

/// fxdax: whether cleaning `store` is due: the segments whose extents hold less than
/// `CLEAN_BELOW` of them hold more than `CLEAN_PAST` bytes no extent is in, or any, once those
/// count as used; and, behind its commits ([`clean_behind`]), no copy of the last pass waits for a
/// commit (TASK-79.2).
pub(crate) fn cleaning_due(store: &Store<HostDir>) -> anyhow::Result<bool> {
    store.cleaning_due(CLEAN_PAST, CLEAN_BELOW)
}

/// fxdax: cleans `store` behind its commits (TASK-79.2): copies what is live of its segments whose
/// extents hold less than `CLEAN_BELOW` of them, `CLEAN_PASS` bytes at most, which its next
/// commit makes the extents' places, and whose flush lets the segments go. Its commits wait for
/// one extent's copy at most, as for a write: so the extent daemon cleans its stores, as their
/// guests write and commit, which, writing faster than cleaning takes back what they free, are
/// refused room ([`extent_store::NoSpace`]).
pub(crate) fn clean_behind(store: &Store<HostDir>) -> anyhow::Result<()> {
    let copied = store.clean_behind(CLEAN_BELOW, CLEAN_PASS)?;
    if copied > 0 {
        log::info!("fxdax: cleaning {}: copied {copied} bytes", store.dir().path().display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_device::ExtentId;

    const MIB: u64 = 1 << 20;

    /// A store is cleaned after a commit once the segments cleaning takes hold more than
    /// `CLEAN_PAST` freed bytes, and then holds in segments only what is live.
    #[test]
    fn stores_clean_once_their_segments_hold_much_freed() {
        let dir = tempfile::tempdir().unwrap();
        let branch = 7;
        let store = create(&dir.path().join("store"), 1 << 30, branch).unwrap();
        let extent = |sequence| ExtentId::new(branch, sequence);
        // A segment of their data, closed by the commit, as it is past a segment's size.
        let extents = CLEAN_PAST / MIB + 8;
        for sequence in 0..extents {
            store.write(extent(sequence), 0, &vec![sequence as u8; MIB as usize]).unwrap();
        }
        store.commit(None).unwrap();

        // Freed, `CLEAN_PAST` bytes are not past it.
        for sequence in 0..CLEAN_PAST / MIB {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(None).unwrap();
        let before = store.usage().unwrap();
        assert_eq!(before.segment_bytes - before.live_bytes, CLEAN_PAST);
        clean_after_commit(&store).unwrap();
        assert_eq!(store.usage().unwrap(), before);

        store.free(extent(CLEAN_PAST / MIB)).unwrap();
        store.commit(None).unwrap();
        clean_after_commit(&store).unwrap();
        let after = store.usage().unwrap();
        assert_eq!((after.segment_bytes, after.live_bytes), (7 * MIB, 7 * MIB));
        let mut data = vec![0; MIB as usize];
        store.read(extent(extents - 1), 0, &mut data).unwrap();
        assert_eq!(data, vec![(extents - 1) as u8; MIB as usize]);
    }

    /// fxdax: a store is cleaned behind its commits once its segments hold more than `CLEAN_PAST`
    /// bytes no extent is in: a pass copies, and commits nothing, and is due no more until the
    /// next commit has made its copies the extents' places; then the segments hold only what is
    /// live (TASK-79.2).
    #[test]
    fn stores_clean_behind_their_commits_once_much_is_freed() {
        let dir = tempfile::tempdir().unwrap();
        let branch = 7;
        let store = create(&dir.path().join("store"), 1 << 30, branch).unwrap();
        let extent = |sequence| ExtentId::new(branch, sequence);
        let extents = CLEAN_PAST / MIB + 8;
        for sequence in 0..extents {
            store.write(extent(sequence), 0, &vec![sequence as u8; MIB as usize]).unwrap();
        }
        store.commit(None).unwrap();
        for sequence in 0..CLEAN_PAST / MIB {
            store.free(extent(sequence)).unwrap();
        }
        store.commit(None).unwrap();
        assert!(!cleaning_due(&store).unwrap());

        store.free(extent(CLEAN_PAST / MIB)).unwrap();
        store.commit(None).unwrap();
        assert!(cleaning_due(&store).unwrap());
        let epoch = store.epoch().unwrap();
        clean_behind(&store).unwrap();
        assert_eq!(store.epoch().unwrap(), epoch);
        assert!(!cleaning_due(&store).unwrap());
        store.commit(None).unwrap();
        let after = store.usage().unwrap();
        assert_eq!((after.segment_bytes, after.live_bytes), (7 * MIB, 7 * MIB));
        assert!(!cleaning_due(&store).unwrap());
        let mut data = vec![0; MIB as usize];
        store.read(extent(extents - 1), 0, &mut data).unwrap();
        assert_eq!(data, vec![(extents - 1) as u8; MIB as usize]);
    }

    /// Deleting a fork removes the record of its branch's placement, whichever stack deletes it.
    #[test]
    fn deleting_a_fork_removes_its_placement() {
        let dir = tempfile::tempdir().unwrap();
        let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
        let store = create(&path, MIB, 3).unwrap();
        extent_store::fork_host(&store, &fork, 4).unwrap();
        crate::ExtentStore::place(&fork, 9).unwrap();
        let canonical = path.canonicalize().unwrap();
        assert_eq!(crate::remote::placed_token(&canonical, 4), Ok(Some(9)));
        crate::Storage::delete(&fork).unwrap();
        assert!(!fork.exists());
        assert_eq!(crate::remote::placed_token(&canonical, 4), Ok(None));
        drop(store);
        assert_eq!(
            crate::Storage::delete(&fork).err().and_then(|err| err.errno()),
            Some(Errno::ENOENT)
        );
    }

    /// A fork's handle whose deletion stopped once its description was gone is removed by
    /// deleting it again.
    #[test]
    fn deleting_an_emptied_handle_finishes_it() {
        let dir = tempfile::tempdir().unwrap();
        let handle = dir.path().join("fork");
        std::fs::create_dir(&handle).unwrap();
        delete(&handle, None).unwrap();
        assert!(!handle.exists());
    }

    /// TASK-79.20: a fork's handle whose description was never put in place, its host crashing
    /// while `extent_store::fork_host` wrote it, before the fork, is removed by deleting it, as
    /// extentd and fxdaxctl delete it.
    #[test]
    fn deleting_a_handle_never_described_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let handle = dir.path().join("fork");
        std::fs::create_dir(&handle).unwrap();
        // The description written, not yet renamed into place.
        std::fs::write(handle.join("store.new"), "fxdax extent store handle\n").unwrap();
        delete(&handle, None).unwrap();
        assert!(!handle.exists());
    }

    /// TASK-79.71: a fork's deletion that stopped as it removed the records of its branch's
    /// placement and uploads from the store's directory, its branch deleted, is finished by
    /// deleting again, which leaves none of them there, nor one a crash left half written. Here
    /// the marker of its uploads is a directory, which removing it as a file fails on, until it is
    /// the file it was again, as the deletion stopped left it.
    #[test]
    fn deletions_stopped_at_a_branchs_records_finish() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let (path, fork) = (dir.path().join("store"), dir.path().join("fork"));
        let store = create(&path, MIB, 3).unwrap();
        // Something for the fork's uploader to upload, which its marker names.
        store.write(ExtentId::new(3, 0), 0, &[7; BLOCK_SIZE as usize]).unwrap();
        store.commit(None).unwrap();
        extent_store::fork_host(&store, &fork, 4).unwrap();
        crate::ExtentStore::place(&fork, 9).unwrap();
        let memory = Arc::new(object_store::memory::InMemory::new());
        let bucket = Arc::new(extent_remote::store::Store::new(memory, ""));
        let forked = crate::ExtentStore::open(&fork).unwrap();
        forked.attach_remote(bucket.clone(), 1, crate::remote::Options::default()).unwrap();
        drop(forked);
        let canonical = path.canonicalize().unwrap();
        // The files of the store's directory that name the fork's branch.
        let named = || {
            let names = std::fs::read_dir(&canonical).unwrap();
            let names = names.map(|entry| entry.unwrap().file_name().into_string().unwrap());
            let mut names: Vec<_> =
                names.filter(|name| name.contains(&format!("{:016x}", 4))).collect();
            names.sort();
            names
        };
        let marker = canonical.join("remote-0000000000000004");
        // The marker's next, half written, its host crashing before it went in place.
        std::fs::write(canonical.join("remote-0000000000000004.new"), "1").unwrap();
        assert_eq!(
            named(),
            ["fence-0000000000000004", "remote-0000000000000004", "remote-0000000000000004.new"]
        );

        let marked = std::fs::read(&marker).unwrap();
        std::fs::remove_file(&marker).unwrap();
        std::fs::create_dir(&marker).unwrap();
        let stopped = delete(&fork, Some(&bucket)).map_err(|err| err.to_string());
        assert_eq!(stopped, Err(format!("removing {}", marker.display())));
        std::fs::remove_dir(&marker).unwrap();
        std::fs::write(&marker, marked).unwrap();
        let resumed = delete(&fork, Some(&bucket)).map_err(|err| err.to_string());
        assert_eq!((resumed, fork.exists(), named()), (Ok(()), false, Vec::<String>::new()));
    }
}
