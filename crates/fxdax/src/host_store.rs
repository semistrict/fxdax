// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extent stores in the host's directories, as both of fxdax's host stacks keep them: the images
//! [`Storage`](crate::Storage) mounts, and the stores [`ExtentStore`](crate::ExtentStore) serves a
//! guest. Either makes, opens and deletes its stores here, so that both do it alike. Images also
//! clean theirs here after commits ([`clean_after_commit`]); the daemon's stores don't clean yet.
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

/// The freed bytes in a store's segments past which a commit cleans it, if they are also more than
/// a quarter of the segments' bytes.
const CLEAN_PAST: u64 = 64 << 20;

/// The fraction of a segment its extents must hold for cleaning to leave it be.
const CLEAN_BELOW: f64 = 0.5;

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
/// on; the records of its branch's placement and uploads go with it either way. A deletion that
/// stopped partway is finished by deleting again.
///
/// Fails with `ENOENT` if nothing is at `path`, and with `ESTALE` if a later placement of the
/// store holds its branch in `bucket`.
pub(crate) fn delete(
    path: &Path,
    bucket: Option<&extent_remote::store::Store>,
) -> Result<(), Error> {
    if !path.exists() {
        return Err(Errno::ENOENT.into());
    }
    let resolved = match extent_store::resolve_host(path) {
        Ok(resolved) => resolved,
        // A fork's handle whose deletion stopped once its description was gone, which names
        // nothing any more: its branch went before its description did.
        Err(_) if std::fs::read_dir(path).map_err(Error::new)?.next().is_none() => {
            return extent_store::delete_host(path, options()).map_err(Error::new);
        }
        Err(err) => return Err(Error::new(err)),
    };
    let (dir, branch) = resolved;
    // Its branch there goes once it is found open nowhere here, before it does, as this copy's
    // placement, or this host's, deletes it: not from under a later one (`ESTALE`).
    extent_store::delete_host_after(path, options(), |branch| match bucket {
        Some(bucket) => {
            let placed = crate::remote::placed_token(&dir, branch)?;
            let token = placed.map_or_else(crate::remote::host_token, Ok)?;
            Ok(crate::remote::delete_branch(bucket, branch, token)?)
        }
        None => Ok(()),
    })
    .map_err(|err| {
        let stale = err.chain().any(|cause| {
            cause.downcast_ref::<crate::remote::Failure>() == Some(&crate::remote::Failure::Stale)
        });
        if stale { Error::new(err.context(Errno::ESTALE)) } else { Error::new(err) }
    })?;
    if dir.exists() {
        crate::remote::unmark(&dir, branch).map_err(Error::new)?;
    }
    Ok(())
}

/// Cleans `store`, just committed, if its segments hold much freed space: more than `CLEAN_PAST`
/// bytes, and more than a quarter of their bytes. Cleaning rewrites those whose extents hold less
/// than `CLEAN_BELOW` of them, and commits.
///
/// Images clean so after each of their device's commits. The extent daemon's stores don't yet:
/// cleaning holds a store's commits back while it copies, which would stall its guest's.
pub(crate) fn clean_after_commit(store: &Store<HostDir>) -> anyhow::Result<()> {
    let usage = store.usage()?;
    let freed = usage.segment_bytes - usage.live_bytes;
    if freed > CLEAN_PAST && freed > usage.segment_bytes / 4 {
        let cleaned = store.clean(CLEAN_BELOW)?;
        log::info!("fxdax: cleaned {}: {cleaned:?}", store.dir().path().display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage_device::ExtentId;

    const MIB: u64 = 1 << 20;

    /// A store is cleaned after a commit once its segments hold more than `CLEAN_PAST` freed
    /// bytes, and then holds in segments only what is live.
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
}
