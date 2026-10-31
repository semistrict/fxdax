// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a follower's mount (backlog doc-8): Fxfs, read-only, on a device that follows a store
//! another VM writes, applying the writer's commits as they come, while readers read.
//!
//! The concurrency protocol (crates/follower, TASK-42.5, TASK-42.10) keeps readers and the
//! follower's batches apart with the kernel's own rwsem, the follower's fill barrier, which
//! fs/fxfs's C keeps (`ffi`, vfs.h): its readers hold it shared from before they read the trees
//! until what they read is in the kernel's caches, or copied out; a batch holds it exclusively
//! while it drops what the kernel caches of what it changes, under each inode's invalidate_lock (a
//! file's data ranges, mapped or cached; a directory's dentries, which revalidate), applies its
//! commits, and has the kernel's inodes take the attributes they left, which the apply reads.
//!
//! The files the kernel has open are held (`Fxfs::hold`): one the writer deletes reads as it was
//! until the last close, as POSIX has it (doc-8's graveyard).
//!
//! A batch that fails ends the follower: what reads the trees fails with ESTALE (`follower::Failed`)
//! until it is remounted, which opens the store anew.

use crate::fs::{Attr, Fxfs, GuestFxfs};
use anyhow::{Error, ensure};
use core::ffi::c_void;
use follower::{Follow, Kernel, Protocol};
use fxfs::follower::{Batch, Changes, Hold, StoreObject};
use rustc_hash::FxHashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use storage_device::Previous;

mod ffi {
    use super::Attr;
    use core::ffi::c_void;

    unsafe extern "C" {
        pub fn fxfs_vfs_follow_fill_begin(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_fill_end(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_assert_filling(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_barrier_take(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_barrier_release(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_epoch(vfs: *mut c_void, which: u32, epoch: u64);
        pub fn fxfs_vfs_follow_failing() -> bool;
        pub fn fxfs_vfs_drop_file(vfs: *mut c_void, ino: u64, ranges: *const Range, count: usize);
        pub fn fxfs_vfs_drop_entries(vfs: *mut c_void, ino: u64);
        pub fn fxfs_vfs_settle_file(vfs: *mut c_void, ino: u64, attr: *const Attr, gone: bool);
    }

    /// `struct fxfs_range`: bytes of a file a batch changed, from `start` to `end`, exclusive.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Range {
        pub start: u64,
        pub end: u64,
    }
}

/// The follower's state is as of a commit no later than this epoch's: its batch applied.
const FOLLOW_APPLIED: u32 = 0;
/// The follower's state is as of a commit no earlier than this epoch's: its batch settled.
const FOLLOW_SETTLED: u32 = 1;

/// The kernel's primitives, as fs/fxfs's C has them for the superblock `vfs`, for the volume
/// whose store is `volume`.
pub struct GuestKernel {
    vfs: *mut c_void,
    volume: u64,
    /// Whether a batch failed, set holding the fill barrier, which readers hold as they look.
    failed: AtomicBool,
}

// SAFETY: The superblock's primitives are the kernel's, which any thread uses.
unsafe impl Send for GuestKernel {}
// SAFETY: As above.
unsafe impl Sync for GuestKernel {}

/// What a batch changed, the attributes its apply left the objects it changed with, and the epoch
/// of the commit it applied.
pub struct GuestChanges {
    changes: Changes,
    attrs: Mutex<Vec<(u64, Option<Attr>)>>,
    epoch: AtomicU64,
}

impl GuestChanges {
    /// The objects of the volume `volume` whose data, attributes or life changed.
    fn objects(&self, volume: u64) -> BTreeSet<u64> {
        let changes = &self.changes;
        changes
            .attributes
            .iter()
            .chain(changes.data.keys())
            .chain(&changes.deleted)
            .filter(|object| object.store_object_id == volume)
            .map(|object| object.object_id)
            .collect()
    }
}

impl GuestKernel {
    /// Says the follower's state is as of `epoch`'s commit, as `which` says (FOLLOW_*).
    fn epoch(&self, which: u32, epoch: u64) {
        // SAFETY: The superblock outlives the filesystem.
        unsafe { ffi::fxfs_vfs_follow_epoch(self.vfs, which, epoch) };
    }

    /// Checks, in a debug kernel (lockdep), that the current task holds the fill barrier: as a
    /// read of the trees must, on behalf of a reader.
    pub fn assert_filling(&self) {
        // SAFETY: The superblock outlives the filesystem.
        unsafe { ffi::fxfs_vfs_follow_assert_filling(self.vfs) };
    }

    fn ours<'a>(&self, objects: impl IntoIterator<Item = &'a StoreObject>) -> Vec<u64> {
        let objects = objects.into_iter().filter(|object| object.store_object_id == self.volume);
        objects.map(|object| object.object_id).collect()
    }
}

impl Kernel for GuestKernel {
    type Changes = Arc<GuestChanges>;

    fn fill_begin(&self) {
        // SAFETY: The superblock outlives the filesystem.
        unsafe { ffi::fxfs_vfs_follow_fill_begin(self.vfs) }
    }

    fn fill_end(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_fill_end(self.vfs) }
    }

    fn barrier_take(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_barrier_take(self.vfs) }
    }

    fn barrier_release(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_barrier_release(self.vfs) }
    }

    fn drop_cached(&self, changes: &Arc<GuestChanges>) {
        // Each file's ranges together: its data's, and, deleted, all of it.
        let mut files: BTreeMap<u64, Vec<ffi::Range>> = BTreeMap::new();
        let ours = |object: &&StoreObject| object.store_object_id == self.volume;
        for (object, ranges) in changes.changes.data.iter().filter(|(object, _)| ours(object)) {
            let file = files.entry(object.object_id).or_default();
            file.extend(ranges.iter().map(|r| ffi::Range { start: r.start, end: r.end }));
        }
        for ino in self.ours(&changes.changes.deleted) {
            files.entry(ino).or_default().push(ffi::Range { start: 0, end: u64::MAX });
        }
        for (ino, ranges) in &files {
            // SAFETY: The superblock outlives the filesystem, and `ranges` the call.
            unsafe { ffi::fxfs_vfs_drop_file(self.vfs, *ino, ranges.as_ptr(), ranges.len()) };
        }
        for ino in self.ours(&changes.changes.entries) {
            // SAFETY: The superblock outlives the filesystem.
            unsafe { ffi::fxfs_vfs_drop_entries(self.vfs, ino) };
        }
        if changes.changes.volumes_deleted.contains(&self.volume) {
            log::error!("the writer deleted the volume this follower follows");
        }
    }

    fn settle(&self, changes: &Arc<GuestChanges>) {
        for (ino, attr) in changes.attrs.lock().unwrap().iter() {
            let gone = attr.is_none();
            let attr = attr.as_ref().map_or(core::ptr::null(), |attr| attr as *const Attr);
            // SAFETY: The superblock outlives the filesystem, and `attr` the call.
            unsafe { ffi::fxfs_vfs_settle_file(self.vfs, *ino, attr, gone) };
        }
        self.epoch(FOLLOW_SETTLED, changes.epoch.load(Ordering::Acquire));
    }

    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// A follower's state: the protocol its readers and batches keep, and the files the kernel has
/// open, held.
pub struct Following {
    protocol: Protocol<GuestKernel>,
    holds: Mutex<FxHashMap<u64, (u32, Hold)>>,
    /// The epoch of the commit its device last advanced to.
    advanced: Arc<AtomicU64>,
}

impl Following {
    /// A follower's state, for the volume whose store is `volume`, on the superblock `vfs`, its
    /// device advancing as `advanced` says, as of a commit no later than `latest`'s.
    pub fn new(vfs: *mut c_void, volume: u64, advanced: Arc<AtomicU64>, latest: u64) -> Self {
        let kernel = GuestKernel { vfs, volume, failed: AtomicBool::new(false) };
        kernel.epoch(FOLLOW_APPLIED, latest);
        Self { protocol: Protocol::new(kernel), holds: Mutex::default(), advanced }
    }

    pub fn protocol(&self) -> &Protocol<GuestKernel> {
        &self.protocol
    }

    /// Holds the file `ino`, which the kernel opens, with `hold` if it isn't yet.
    pub fn hold(&self, ino: u64, hold: impl FnOnce() -> Hold) {
        let mut holds = self.holds.lock().unwrap();
        match holds.get_mut(&ino) {
            Some((opens, _)) => *opens += 1,
            None => {
                holds.insert(ino, (1, hold()));
            }
        }
    }

    /// Lets go of the file `ino`, which the kernel closes, once its last open is.
    pub fn release(&self, ino: u64) {
        let mut holds = self.holds.lock().unwrap();
        match holds.get_mut(&ino) {
            Some((opens, _)) if *opens > 1 => *opens -= 1,
            Some(_) => {
                holds.remove(&ino);
            }
            None => log::error!("inode {ino} closed but never held"),
        }
    }
}

/// The follower's Fxfs, as the protocol follows it.
pub struct GuestFollow<'a>(pub &'a Fxfs);

impl GuestFollow<'_> {
    /// Reads the attributes a batch left the objects it changed with, what the kernel's inodes
    /// take as it settles, and says its commit is applied.
    async fn applied(&self, changes: &GuestChanges) -> Result<(), Error> {
        // SAFETY: Reads a module parameter.
        ensure!(!unsafe { ffi::fxfs_vfs_follow_failing() }, "a test failed the batch");
        let mut attrs = Vec::new();
        for ino in changes.objects(self.0.volume_id()) {
            attrs.push((ino, self.0.attr_applied(ino).await?));
        }
        *changes.attrs.lock().unwrap() = attrs;
        // The batch's commit: what the device advanced to as it was fetched.
        let following = self.0.following().expect("a follower's batch");
        let epoch = following.advanced.load(Ordering::Acquire);
        changes.epoch.store(epoch, Ordering::Release);
        following.protocol.kernel().epoch(FOLLOW_APPLIED, epoch);
        Ok(())
    }
}

impl Follow for GuestFollow<'_> {
    type Changes = Arc<GuestChanges>;
    type Batch = (Batch, Arc<GuestChanges>);
    type Previous = Previous;

    async fn fetch(&self) -> Result<(Self::Batch, Self::Changes), Error> {
        let (batch, changes) = self.0.filesystem().fetch().await?;
        let changes =
            Arc::new(GuestChanges { changes, attrs: Mutex::default(), epoch: AtomicU64::new(0) });
        Ok(((batch, changes.clone()), changes))
    }

    async fn apply(&self, (batch, changes): Self::Batch) -> Result<Previous, Error> {
        let filesystem = self.0.filesystem();
        let previous = filesystem.apply(batch).await?;
        match self.applied(&changes).await {
            Ok(()) => Ok(previous),
            Err(error) => {
                filesystem.stop_following(previous);
                Err(error)
            }
        }
    }
}
