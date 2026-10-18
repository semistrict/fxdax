// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: a follower's mount (backlog doc-8): Fxfs, read-only, on a device that follows a store
//! another VM writes, applying the writer's commits as they come, while readers read.
//!
//! The concurrency protocol (crates/follower, TASK-42.5) keeps readers and the follower's
//! batches apart, on the kernel's own primitives, which fs/fxfs's C implements (`ffi`, vfs.h): a
//! seqcount_t each tree read validates against, which a batch writes holding the follower's mutex,
//! which a read falls back to, and SRCU, whose grace period comes before the follower lets go of
//! the commit it was as of. After a batch, the kernel's caches of what it changed are invalidated, under each
//! inode's locks: a file's data ranges, mapped or cached; its attributes, which the batch's apply
//! reads, inside the seqcount, and the kernel's inode takes after; and directories' entries,
//! whose dentries revalidate.
//!
//! The files the kernel has open are held (`Fxfs::hold`): one the writer deletes reads as it was
//! until the last close, as POSIX has it (doc-8's graveyard).

use crate::fs::{Attr, Fxfs};
use anyhow::Error;
use core::ffi::{c_int, c_void};
use follower::{Follow, Kernel, Protocol};
use fxfs::follower::{Batch, Changes, Hold, StoreObject};
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use storage_device::Previous;

mod ffi {
    use super::Attr;
    use core::ffi::{c_int, c_void};

    unsafe extern "C" {
        pub fn fxfs_vfs_follow_sequence(vfs: *mut c_void) -> u32;
        pub fn fxfs_vfs_follow_moved(vfs: *mut c_void, start: u32) -> bool;
        pub fn fxfs_vfs_follow_write_begin(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_write_end(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_lock(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_unlock(vfs: *mut c_void);
        pub fn fxfs_vfs_follow_grace_enter(vfs: *mut c_void) -> c_int;
        pub fn fxfs_vfs_follow_grace_leave(vfs: *mut c_void, index: c_int);
        pub fn fxfs_vfs_follow_grace_wait(vfs: *mut c_void);
        pub fn fxfs_vfs_changed(
            vfs: *mut c_void,
            what: u32,
            ino: u64,
            start: u64,
            end: u64,
            attr: *const Attr,
        );
    }
}

// What a batch changed of an inode, as `fxfs_vfs_changed` takes it (vfs.h's FXFS_CHANGED_*).
/// Its data from `start` to `end`.
const CHANGED_DATA: u32 = 0;
/// Its attributes, to the `attr` given, or none if it is gone.
const CHANGED_ATTR: u32 = 1;
/// A directory's entries.
const CHANGED_ENTRIES: u32 = 2;
/// The object is deleted: unlinked from its last directory, or tombstoned.
const CHANGED_DELETED: u32 = 3;

/// How often a tree read is tried before it is made under the follower's mutex.
const TRIES: u32 = 4;

/// The kernel's primitives, as fs/fxfs's C has them for the superblock `vfs`, for the volume
/// whose store is `volume`.
pub struct GuestKernel {
    vfs: *mut c_void,
    volume: u64,
}

// SAFETY: The superblock's primitives are the kernel's, which any thread uses.
unsafe impl Send for GuestKernel {}
// SAFETY: As above.
unsafe impl Sync for GuestKernel {}

/// What a batch changed, and the attributes its apply left the objects it changed with.
pub struct GuestChanges {
    changes: Changes,
    attrs: Mutex<Vec<(u64, Option<Attr>)>>,
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
    fn changed(&self, what: u32, ino: u64, start: u64, end: u64, attr: Option<&Attr>) {
        let attr = attr.map_or(core::ptr::null(), |attr| attr as *const Attr);
        // SAFETY: The superblock outlives the filesystem, and `attr` the call.
        unsafe { ffi::fxfs_vfs_changed(self.vfs, what, ino, start, end, attr) };
    }

    fn ours<'a>(&self, objects: impl IntoIterator<Item = &'a StoreObject>) -> Vec<u64> {
        let objects = objects.into_iter().filter(|object| object.store_object_id == self.volume);
        objects.map(|object| object.object_id).collect()
    }
}

impl Kernel for GuestKernel {
    type Changes = Arc<GuestChanges>;

    fn sequence(&self) -> u32 {
        // SAFETY: The superblock outlives the filesystem.
        unsafe { ffi::fxfs_vfs_follow_sequence(self.vfs) }
    }

    fn moved(&self, start: u32) -> bool {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_moved(self.vfs, start) }
    }

    fn write_begin(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_write_begin(self.vfs) }
    }

    fn write_end(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_write_end(self.vfs) }
    }

    fn lock(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_lock(self.vfs) }
    }

    fn unlock(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_unlock(self.vfs) }
    }

    fn grace_enter(&self) -> u32 {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_grace_enter(self.vfs) as u32 }
    }

    fn grace_leave(&self, index: u32) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_grace_leave(self.vfs, index as c_int) }
    }

    fn grace_wait(&self) {
        // SAFETY: As above.
        unsafe { ffi::fxfs_vfs_follow_grace_wait(self.vfs) }
    }

    fn invalidate(&self, changes: &Arc<GuestChanges>) {
        let ours = |object: &&StoreObject| object.store_object_id == self.volume;
        for (object, ranges) in changes.changes.data.iter().filter(|(object, _)| ours(object)) {
            for range in ranges {
                self.changed(CHANGED_DATA, object.object_id, range.start, range.end, None);
            }
        }
        for ino in self.ours(&changes.changes.deleted) {
            self.changed(CHANGED_DELETED, ino, 0, u64::MAX, None);
        }
        for (ino, attr) in changes.attrs.lock().unwrap().iter() {
            self.changed(CHANGED_ATTR, *ino, 0, 0, attr.as_ref());
        }
        for ino in self.ours(&changes.changes.entries) {
            self.changed(CHANGED_ENTRIES, ino, 0, 0, None);
        }
        if changes.changes.volumes_deleted.contains(&self.volume) {
            log::error!("the writer deleted the volume this follower follows");
        }
    }
}

/// A follower's state: the protocol its readers and batches keep, and the files the kernel has
/// open, held.
pub struct Following {
    protocol: Protocol<GuestKernel>,
    holds: Mutex<FxHashMap<u64, (u32, Hold)>>,
}

impl Following {
    /// A follower's state, for the volume whose store is `volume`, on the superblock `vfs`.
    pub fn new(vfs: *mut c_void, volume: u64) -> Self {
        Self { protocol: Protocol::new(GuestKernel { vfs, volume }, TRIES), holds: Mutex::default() }
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

impl Follow for GuestFollow<'_> {
    type Changes = Arc<GuestChanges>;
    type Batch = (Batch, Arc<GuestChanges>);
    type Previous = Previous;

    async fn fetch(&self) -> Result<(Self::Batch, Self::Changes), Error> {
        let (batch, changes) = self.0.filesystem().fetch().await?;
        let changes = Arc::new(GuestChanges { changes, attrs: Mutex::default() });
        Ok(((batch, changes.clone()), changes))
    }

    /// Applies the batch, and reads the attributes it left the objects it changed with, inside
    /// the seqcount: what the kernel's inodes take as the batch is invalidated.
    async fn apply(&self, (batch, changes): Self::Batch) -> Result<Previous, Error> {
        let previous = self.0.filesystem().apply(batch).await?;
        let mut attrs = Vec::new();
        for ino in changes.objects(self.0.volume_id()) {
            attrs.push((ino, self.0.attr_applied(ino).await));
        }
        *changes.attrs.lock().unwrap() = attrs;
        Ok(previous)
    }
}
