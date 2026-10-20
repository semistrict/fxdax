// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Fuchsia's `src/storage/fxfs/src/object_store/allocator.rs`, over an extent store (backlog doc-4,
// TASK-26.3). The device holds extents, named by IDs the allocator mints: one per allocation, of
// the device's branch and a sequence number the allocator keeps (AllocatorInfoV60). The tree holds
// each extent's ranges with their owners: allocated (count 1), or deallocated while the rest of the
// extent is not (count 0), so that the extent's size is known until it dies. An extent dies when
// its last range is deallocated, or its owner deleted; the allocator frees it on the device once
// the device has been flushed (see "Dead extents" below). There is no free space to track: no
// allocation strategy, no temporary allocations, and no trimming. Capacity counts whole extents,
// dead or alive, until they are freed. fxdax snapshots (volume snapshots that share their source's
// layers and extents) deviate from upstream too: allocations are recorded under a store's current
// data owner, which a snapshot changes; a snapshot freezes the source's owner, whose extents the
// stores sharing them never deallocate one by one, and which is deleted once no store shares it
// (AllocatorInfoV59, the Snapshot mutation, `deallocate`, `is_shared`, `frozen_ranges`).

//! # The Allocator
//!
//! The allocator in Fxfs is filesystem-wide entity responsible for managing the allocation of
//! extents of the device to "owners" (which are `ObjectStore`).
//!
//! Allocations are tracked in an LSMTree with coalescing used to merge neighboring allocations
//! with the same properties (owner and reference count).
//!
//! There are a number of important implementation features of the allocator that warrant further
//! documentation.
//!
//! ## Byte limits
//!
//! Fxfs is a multi-volume filesystem. Fuchsia with fxblob currently uses two primary volumes -
//! an unencrypted volume for blob storage and an encrypted volume for data storage.
//! Byte limits ensure that no one volume can consume all available storage. This is important
//! as software updates must always be possible (blobs) and conversely configuration data should
//! always be writable (data).
//!
//! ## Reservation tracking
//!
//! Fxfs on Fuchsia leverages write-back caching which allows us to buffer writes in RAM until
//! memory pressure, an explicit flush or a file close requires us to persist data to storage.
//!
//! To ensure that we do not over-commit in such cases (e.g. by writing many files to RAM only to
//! find out tha there is insufficient disk to store them all), the allocator includes a simple
//! reservation tracking mechanism.
//!
//! Reservation tracking is implemented hierarchically such that a reservation can portion out
//! sub-reservations, each of which may be "reserved" or "forgotten" when it comes time to actually
//! allocate space.
//!
//! ## Fixed locations
//!
//! The only fixed location files in Fxfs are the first 512kiB extents of the two superblocks, in
//! extents no allocator mints (`SUPER_BLOCK_A_EXTENT` and `SUPER_BLOCK_B_EXTENT`). The allocator
//! manages these extents, but does so using a 'mark_allocated' method distinct from all other
//! allocations, whose extents the allocator mints.
//!
//! ## Dead extents
//!
//! An extent is not freed on the device until after a flush. Transactions are not guaranteed to be
//! persisted until after a successful flush, so freeing an extent before then, followed by power
//! loss, may leave the filesystem referring to an extent that is gone.
//!
//! Dead extents are tracked in RAM until the device is flushed. The device's capacity counts them
//! until they are freed: when they stand in the way of an allocation, the allocator flushes.
//!
//! Extents whose transactions are dropped die too, as their data may have been written.
//!
//! A free is on the device only once a later flush is: until then, a crash may undo it. An extent
//! that dies leaves the tree, so a compaction of the tree, once on the device, would leave nothing
//! to find such an extent by. Each compaction keeps the extents dead or freed since the last flush,
//! which might yet be on the device, in AllocatorInfo (`dying`), and a mount frees them again.
//!
//! ## Allocated but uncommitted extents
//!
//! These are allocated extents that are not (yet) persisted to disk. They are regular
//! file allocations, but are not stored on persistent storage until their transaction is committed
//! and safely flushed to disk.
//!
//! ## Shared extents (fxdax)
//!
//! Small allocations share extents, so that a directory of many small files doesn't take an extent
//! each (`allocate_shared`): they take the next blocks of the staging extent, minted for them and
//! open until the device's next barrier seals it, or until it is full, when the next is minted.
//! Such an extent dies, as any does, once its last range is deallocated; but not while it is the
//! staging extent, or while transactions not yet committed have allocated in it (it is pinned): it
//! dies, if nothing is allocated in it, once neither holds. Its dead ranges stay stored until it
//! does (backlog doc-4).
//!
//! ## Orphaned extents
//!
//! A crash may leave extents on the device whose transactions were lost. Their sequence numbers
//! follow those of every allocation the journal holds, and precede the device's next sequence
//! number: after replay, the allocator frees them, and mints from there.
//!
//! ## Volume deletion
//!
//! We make use of an optimisation in the case where an entire volume is deleted. In such cases,
//! rather than individually deallocate all extents associated with that volume, we make note of
//! the deletion and perform special merge operation on the next LSMTree compaction that filters out
//! allocations for the deleted volume. Its extents die at once; a compaction filters out an owner's
//! allocations only once its extents have been freed.
pub mod merge;

use crate::errors::FxfsError;
use crate::filesystem::{ApplyContext, ApplyMode, FxFilesystem, JournalingObject, SyncOptions};
use crate::log::*;
use crate::lsm_tree::types::{
    FuzzyHash, Item, ItemRef, LayerIterator, LayerKey, MergeType, OrdLowerBound, OrdUpperBound,
    SortByU64, Value,
};
use crate::lsm_tree::{LSMTree, Query, compact_with_iterator, layer_from_handle, open_layers};
use crate::object_handle::{INVALID_OBJECT_ID, ObjectHandle, ReadObjectHandle};
use crate::object_store::object_manager::ReservationUpdate;
use crate::object_store::transaction::{
    AllocatorMutation, AssocObj, LockKey, Mutation, Options, Transaction, WriteGuard, lock_keys,
};
use crate::object_store::{
    DataObjectHandle, DirectWriter, Extent, HandleOptions, ObjectStore, ReservedId, tree,
};
use crate::range::RangeExt;
use crate::round::round_div;
use crate::serialized_types::{
    DEFAULT_MAX_SERIALIZED_RECORD_SIZE, LATEST_VERSION, Migrate, Version, Versioned,
    VersionedLatest, migrate_to_version,
};
use anyhow::{Context, Error, anyhow, bail, ensure};
use async_trait::async_trait;
use either::Either::{Left, Right};
use fprint::TypeFingerprint;
use fuchsia_async as fasync;
use fuchsia_inspect::HistogramProperty;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use futures::future::BoxFuture;
use fxfs_macros::SerializeKey;
use merge::{filter_marked_for_deletion, filter_tombstones, merge};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::marker::PhantomData;
use std::num::{NonZero, Saturating};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use storage_device::{ExtentId, ExtentRange};
use storage_units::BlockSize;

/// This trait is implemented by things that own reservations.
pub trait ReservationOwner: Send + Sync {
    /// Report that bytes are being released from the reservation back to the the |ReservationOwner|
    /// where |owner_object_id| is the owner under the root object store associated with the
    /// reservation.
    fn release_reservation(&self, owner_object_id: Option<u64>, amount: u64);
}

/// A reservation guarantees that when it comes time to actually allocate, it will not fail due to
/// lack of space.  Sub-reservations (a.k.a. holds) are possible which effectively allows part of a
/// reservation to be set aside until it's time to commit.  Reservations do offer some
/// thread-safety, but some responsibility is born by the caller: e.g. calling `forget` and
/// `reserve` at the same time from different threads is unsafe. Reservations are have an
/// |owner_object_id| which associates it with an object under the root object store that the
/// reservation is accounted against.
pub struct ReservationImpl<T: Borrow<U>, U: ReservationOwner + ?Sized> {
    owner: T,
    owner_object_id: Option<u64>,
    inner: Mutex<ReservationInner>,
    phantom: PhantomData<U>,
}

#[derive(Debug, Default)]
struct ReservationInner {
    // Amount currently held by this reservation.
    amount: u64,

    // Amount reserved by sub-reservations.
    reserved: u64,
}

impl<T: Borrow<U>, U: ReservationOwner + ?Sized> std::fmt::Debug for ReservationImpl<T, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.lock().fmt(f)
    }
}

impl<T: Borrow<U> + Clone + Send + Sync, U: ReservationOwner + ?Sized> ReservationImpl<T, U> {
    pub fn new(owner: T, owner_object_id: Option<u64>, amount: u64) -> Self {
        Self {
            owner,
            owner_object_id,
            inner: Mutex::new(ReservationInner { amount, reserved: 0 }),
            phantom: PhantomData,
        }
    }

    pub fn owner_object_id(&self) -> Option<u64> {
        self.owner_object_id
    }

    /// Returns the total amount of the reservation, not accounting for anything that might be held.
    pub fn amount(&self) -> u64 {
        self.inner.lock().amount
    }

    /// Adds more to the reservation.
    pub fn add(&self, amount: u64) {
        self.inner.lock().amount += amount;
    }

    /// Returns the entire amount of the reservation.  The caller is responsible for maintaining
    /// consistency, i.e. updating counters, etc, and there can be no sub-reservations (an assert
    /// will fire otherwise).
    pub fn forget(&self) -> u64 {
        let mut inner = self.inner.lock();
        assert_eq!(inner.reserved, 0);
        std::mem::take(&mut inner.amount)
    }

    /// Takes some of the reservation.  The caller is responsible for maintaining consistency,
    /// i.e. updating counters, etc.  This will assert that the amount being forgotten does not
    /// exceed the available reservation amount; the caller should ensure that this is the case.
    pub fn forget_some(&self, amount: u64) {
        let mut inner = self.inner.lock();
        inner.amount -= amount;
        assert!(inner.reserved <= inner.amount);
    }

    /// Returns a partial amount of the reservation. |amount| is passed the |limit| and should
    /// return the amount, which can be zero.
    fn reserve_with(&self, amount: impl FnOnce(u64) -> u64) -> ReservationImpl<&Self, Self> {
        let mut inner = self.inner.lock();
        let taken = amount(inner.amount - inner.reserved);
        inner.reserved += taken;
        ReservationImpl::new(self, self.owner_object_id, taken)
    }

    /// Reserves *exactly* amount if possible.
    pub fn reserve(&self, amount: u64) -> Option<ReservationImpl<&Self, Self>> {
        let mut inner = self.inner.lock();
        if inner.amount - inner.reserved < amount {
            None
        } else {
            inner.reserved += amount;
            Some(ReservationImpl::new(self, self.owner_object_id, amount))
        }
    }

    /// Commits a previously reserved amount from this reservation.  The caller is responsible for
    /// ensuring the amount was reserved.
    pub fn commit(&self, amount: u64) {
        let mut inner = self.inner.lock();
        inner.reserved -= amount;
        inner.amount -= amount;
    }

    /// Returns some of the reservation.
    pub fn give_back(&self, amount: u64) {
        self.owner.borrow().release_reservation(self.owner_object_id, amount);
        let mut inner = self.inner.lock();
        inner.amount -= amount;
        assert!(inner.reserved <= inner.amount);
    }

    /// Moves `amount` from this reservation to another reservation.
    pub fn move_to<V: Borrow<W> + Clone + Send + Sync, W: ReservationOwner + ?Sized>(
        &self,
        other: &ReservationImpl<V, W>,
        amount: u64,
    ) {
        assert_eq!(self.owner_object_id, other.owner_object_id());
        let mut inner = self.inner.lock();
        if let Some(amount) = inner.amount.checked_sub(amount) {
            inner.amount = amount;
        } else {
            std::mem::drop(inner);
            panic!("Insufficient reservation space");
        }
        other.add(amount);
    }
}

impl<T: Borrow<U>, U: ReservationOwner + ?Sized> Drop for ReservationImpl<T, U> {
    fn drop(&mut self) {
        let inner = self.inner.get_mut();
        assert_eq!(inner.reserved, 0);
        let owner_object_id = self.owner_object_id;
        if inner.amount > 0 {
            self.owner
                .borrow()
                .release_reservation(owner_object_id, std::mem::take(&mut inner.amount));
        }
    }
}

impl<T: Borrow<U> + Send + Sync, U: ReservationOwner + ?Sized> ReservationOwner
    for ReservationImpl<T, U>
{
    fn release_reservation(&self, owner_object_id: Option<u64>, amount: u64) {
        // Sub-reservations should belong to the same owner (or lack thereof).
        assert_eq!(owner_object_id, self.owner_object_id);
        let mut inner = self.inner.lock();
        assert!(inner.reserved >= amount, "{} >= {}", inner.reserved, amount);
        inner.reserved -= amount;
    }
}

pub type Reservation = ReservationImpl<Arc<dyn ReservationOwner>, dyn ReservationOwner>;

pub type Hold<'a> = ReservationImpl<&'a Reservation, Reservation>;

/// Our allocator implementation tracks extents with a reference count: 1 where they are allocated,
/// and 0 where they were deallocated and the rest of their extent was not.
pub type AllocatorKey = AllocatorKeyV60;

/// A range of bytes within an extent.
#[derive(
    Clone,
    Debug,
    Deserialize,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    TypeFingerprint,
    SerializeKey,
    Versioned,
)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub struct AllocatorKeyV60 {
    pub extent: ExtentId,
    pub range: Extent,
}

impl SortByU64 for AllocatorKey {
    fn get_leading_u64(&self) -> u64 {
        self.extent.sequence
    }
}

const EXTENT_HASH_BUCKET_SIZE: BlockSize = BlockSize::SIZE_1MIB;

/// fxdax: the most bytes a staging extent holds (see "Shared extents").
pub const STAGING_SIZE: u64 = 2 << 20;

/// fxdax: the most bytes one shared allocation takes.
pub const STAGED_MAX: u64 = 256 << 10;

pub struct AllocatorKeyPartitionIterator {
    extent: ExtentId,
    range: Range<u64>,
}

impl Iterator for AllocatorKeyPartitionIterator {
    type Item = u64;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.start >= self.range.end {
            None
        } else {
            let start = self.range.start;
            self.range.start = start.saturating_add(EXTENT_HASH_BUCKET_SIZE.get());
            let end = std::cmp::min(self.range.start, self.range.end);
            let key = AllocatorKey { extent: self.extent, range: Extent(start..end) };
            let hash = crate::stable_hash::stable_hash(key);
            Some(hash)
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = if self.range.start >= self.range.end {
            0
        } else {
            let diff = self.range.end - self.range.start;
            let count = EXTENT_HASH_BUCKET_SIZE.align_up_to_blocks(diff);
            usize::try_from(count).unwrap_or(usize::MAX)
        };
        (len, Some(len))
    }
}

impl ExactSizeIterator for AllocatorKeyPartitionIterator {}

impl FuzzyHash for AllocatorKey {
    fn fuzzy_hash(&self) -> impl ExactSizeIterator<Item = u64> {
        AllocatorKeyPartitionIterator {
            extent: self.extent,
            range: EXTENT_HASH_BUCKET_SIZE.align_down(self.range.start)
                ..EXTENT_HASH_BUCKET_SIZE.align_up(self.range.end).unwrap_or(u64::MAX),
        }
    }

    fn is_range_key(&self) -> bool {
        true
    }
}

impl AllocatorKey {
    pub fn new(extent: ExtentId, range: Range<u64>) -> Self {
        Self { extent, range: Extent(range) }
    }

    /// Returns a search key for `merge_into` that finds any touching predecessor to coalesce with.
    ///
    /// Extents in the tree are sorted by `cmp_upper_bound` as `(end, -start)`:
    ///
    /// ```text
    /// Tree:       [  Predecessor  ) [ Key to insert )
    /// Offsets:    X              10                20
    ///
    /// Search key:                 | (10..10)
    /// ```
    ///
    /// Searching with `10..10` has end=10, len=0. Because shorter extents sort earlier when
    /// ends match, `10..10` sorts before any valid extent `X..10` (len > 0). This positions the
    /// iterator at `X..10` so it can be merged with `10..20`.
    pub fn lower_bound_for_merge_into(&self) -> AllocatorKey {
        AllocatorKey { extent: self.extent, range: self.range.key_for_merge_into() }
    }

    /// The range within the extent, as an `ExtentRange`.
    pub fn extent_range(&self) -> ExtentRange {
        ExtentRange::new(self.extent, self.range.0.clone())
    }

    /// Whether this key's range ends before `other`'s starts: an earlier extent, or an earlier
    /// range of the same extent.
    pub fn is_before(&self, other: &AllocatorKey) -> bool {
        self.extent < other.extent
            || (self.extent == other.extent && self.range.end <= other.range.start)
    }
}

impl LayerKey for AllocatorKey {
    fn merge_type(&self) -> MergeType {
        MergeType::OptimizedMerge
    }

    fn next_key(&self) -> Option<Self> {
        Some(Self { extent: self.extent, range: Extent::search_key_from_offset(self.range.end) })
    }

    fn search_key(&self) -> Option<Self> {
        Some(Self { extent: self.extent, range: self.range.search_key() })
    }

    fn is_search_key(&self) -> bool {
        self.range.is_search_key()
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.extent == other.extent && self.range.overlaps(&other.range)
    }
}

impl OrdUpperBound for AllocatorKey {
    fn cmp_upper_bound(&self, other: &AllocatorKey) -> std::cmp::Ordering {
        // Defer to cmp_upper_bound ordering provided by Extent type which
        // uses (end, len) to order ranges.
        self.extent.cmp(&other.extent).then_with(|| self.range.cmp_upper_bound(&other.range))
    }
}

impl OrdLowerBound for AllocatorKey {
    fn cmp_lower_bound(&self, other: &AllocatorKey) -> std::cmp::Ordering {
        // The ordering over range.end is significant here as it is used in
        // the heap ordering that feeds into our merge function and
        // a total ordering over range lets us remove a symmetry case from
        // the allocator merge function.
        self.extent.cmp(&other.extent).then_with(|| self.range.cmp(&other.range))
    }
}

/// Allocations are "owned" by a single ObjectStore and are reference counted
/// (for future snapshot/clone support).
pub type AllocatorValue = AllocatorValueV32;
impl Value for AllocatorValue {
    const DELETED_MARKER: Self = Self::None;
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TypeFingerprint, Versioned)]
#[cfg_attr(fuzz, derive(arbitrary::Arbitrary))]
pub enum AllocatorValueV32 {
    // Tombstone variant indicating an extent is no longer allocated.
    None,
    // Used when we know there are no possible allocations below us in the stack.
    // This is currently all the time. We used to have a related Delta type but
    // it has been removed due to correctness issues (https://fxbug.dev/42179428).
    // fxdax: a count of 0 is a deallocated range of an extent another range of which is still
    // allocated.
    Abs { count: u64, owner_object_id: u64 },
}

pub type AllocatorItem = Item<AllocatorKey, AllocatorValue>;

/// Serialized information about the allocator.
pub type AllocatorInfo = AllocatorInfoV60;

#[derive(Debug, Default, Clone, Deserialize, Serialize, TypeFingerprint)]
pub struct AllocatorInfoV60 {
    /// Holds the set of layer file object_id for the LSM tree (newest first).
    pub layers: Vec<u64>,
    /// Maps from owner_object_id to bytes allocated.
    pub allocated_bytes: BTreeMap<u64, u64>,
    /// Set of owner_object_id that we should ignore if found in layer files. fxdax: an owner stays
    /// here until a compaction filters out its allocations, which waits for its extents to be
    /// freed.
    pub marked_for_deletion: HashSet<u64>,
    /// The limit for the number of allocates bytes per `owner_object_id` whereas the value. If
    /// there is no limit present here for an `owner_object_id` assume it is max u64.
    pub limit_bytes: BTreeMap<u64, u64>,
    /// fxdax: the current data owner of each store a snapshot gave a new one, by store object id.
    /// A store not here allocates under its own object id.
    pub data_owners: BTreeMap<u64, u64>,
    /// fxdax: the owners snapshots froze, with the number of stores sharing their extents. Their
    /// extents are never deallocated one by one; they are freed once no store shares them.
    pub frozen_owners: BTreeMap<u64, u64>,
    /// fxdax: the frozen owners whose extents each store shares, by store object id.
    pub shared_owners: BTreeMap<u64, Vec<u64>>,
    /// fxdax: one more than the highest sequence number of any extent allocated, of any branch.
    pub next_sequence: u64,
    /// fxdax: the extents that died before the compaction that wrote this, and that the device
    /// might hold, as it might not have their frees, which a mount repeats.
    pub dying: Vec<ExtentId>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, TypeFingerprint, Versioned, Migrate)]
pub struct AllocatorInfoV59 {
    pub layers: Vec<u64>,
    pub allocated_bytes: BTreeMap<u64, u64>,
    pub marked_for_deletion: HashSet<u64>,
    pub limit_bytes: BTreeMap<u64, u64>,
    pub data_owners: BTreeMap<u64, u64>,
    pub frozen_owners: BTreeMap<u64, u64>,
    pub shared_owners: BTreeMap<u64, Vec<u64>>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, TypeFingerprint, Versioned, Migrate)]
#[migrate_to_version(AllocatorInfoV59)]
pub struct AllocatorInfoV32 {
    /// Holds the set of layer file object_id for the LSM tree (newest first).
    pub layers: Vec<u64>,
    /// Maps from owner_object_id to bytes allocated.
    pub allocated_bytes: BTreeMap<u64, u64>,
    /// Set of owner_object_id that we should ignore if found in layer files.  For now, this should
    /// always be empty on-disk because we always do full compactions.
    pub marked_for_deletion: HashSet<u64>,
    /// The limit for the number of allocates bytes per `owner_object_id` whereas the value. If
    /// there is no limit present here for an `owner_object_id` assume it is max u64.
    pub limit_bytes: BTreeMap<u64, u64>,
}

/// fxdax: the info holds the extents dying, as many as freeing files leaves between compactions,
/// which can take more than a record's usual limit: it has none, and is read whole.
impl Versioned for AllocatorInfoV60 {
    fn max_serialized_size() -> Option<u64> {
        None
    }
}

/// An extent for tests whose records need one, of branch 0, which no allocator mints.
#[cfg(test)]
pub(crate) const TEST_EXTENT: ExtentId = ExtentId::new(0, 100);

/// Computes the target maximum extent size based on the block size of the allocator.
pub fn max_extent_size_for_block_size(block_size: BlockSize) -> u64 {
    // Each block in an extent contains an 8-byte checksum (which due to varint encoding is 9
    // bytes), and a given extent record must be no larger DEFAULT_MAX_SERIALIZED_RECORD_SIZE.  We
    // also need to leave a bit of room (arbitrarily, 64 bytes) for the rest of the extent's
    // metadata.
    block_size * ((DEFAULT_MAX_SERIALIZED_RECORD_SIZE - 64) / 9)
}

#[derive(Default)]
struct AllocatorCounters {
    num_flushes: u64,
    last_flush_time: Option<std::time::SystemTime>,
}

pub struct Allocator {
    filesystem: Weak<FxFilesystem>,
    block_size: BlockSize,
    /// The device's capacity. fxdax: it changes while mounted, as the extent store is resized
    /// ([`Allocator::set_device_size`]).
    device_size: AtomicU64,
    /// The branch whose extents the allocator mints. fxdax: it changes as the filesystem thaws in
    /// a VM restored from a snapshot, whose store is a fork of its own ([`Allocator::set_branch`]).
    branch: AtomicU64,
    object_id: u64,
    max_extent_size_bytes: u64,
    tree: LSMTree<AllocatorKey, AllocatorValue>,
    inner: Mutex<Inner>,
    counters: Mutex<AllocatorCounters>,
    allocations_allowed: AtomicBool,
    /// fxdax: the frees of the extents the last device flush let die, in progress
    /// (`did_flush_device`), and of those before them. Held while waiting for them, so that all
    /// who wait do.
    freeing: futures::lock::Mutex<Option<fasync::Task<Result<(), Error>>>>,
}

/// Tracks the different stages of byte allocations for an individual owner.
#[derive(Debug, Default, PartialEq)]
struct ByteTracking {
    /// This value is the up-to-date count of the number of allocated bytes per owner_object_id
    /// whereas the value in `Info::allocated_bytes` is the value as it was when we last flushed.
    /// This field should be regarded as *untrusted*; it can be invalid due to filesystem
    /// inconsistencies, and this is why it is `Saturating<u64>` rather than just `u64`.  Any
    /// computations that use this value should typically propagate `Saturating` so that it is clear
    /// the value is *untrusted*.
    allocated_bytes: Saturating<u64>,

    /// This value is the number of bytes allocated to uncommitted allocations.
    /// (Bytes allocated, but not yet persisted to disk)
    uncommitted_allocated_bytes: u64,

    /// This value is the number of bytes allocated to reservations.
    reserved_bytes: u64,
}

impl ByteTracking {
    // Returns the total number of bytes that are taken either from reservations, allocations or
    // uncommitted allocations.
    fn used_bytes(&self) -> Saturating<u64> {
        self.allocated_bytes + Saturating(self.uncommitted_allocated_bytes + self.reserved_bytes)
    }
}

/// An extent allocated (fxdax).
#[derive(Debug)]
struct ExtentInfo {
    /// The owner its allocations are recorded under.
    owner_object_id: u64,
    /// Its size: where its last range ends, allocated or not.
    size: u64,
    /// The bytes of it still allocated.
    allocated: u64,
    /// Where the journal was when the last of it was deallocated, while it was shared (see
    /// "Shared extents"), which kept it alive.
    deallocated_at: u64,
    /// fxdax: its ranges more than one file's extents reference (reflinks, "References"), with
    /// how many: the rest of what is allocated, once.
    references: RangeCounts,
}

impl ExtentInfo {
    fn new(owner_object_id: u64) -> Self {
        Self { owner_object_id, size: 0, allocated: 0, deallocated_at: 0, references: RangeCounts::default() }
    }

    /// fxdax: the parts of `range`, which is allocated, with how many references each has.
    fn references(&self, range: Range<u64>) -> Vec<(Range<u64>, u64)> {
        let mut parts = self.references.counts(range);
        for (_, count) in &mut parts {
            *count = (*count).max(1);
        }
        parts
    }
}

/// fxdax: counts of an extent's ranges, which don't overlap, by where they start: a range not in
/// it counts none.
#[derive(Debug, Default)]
pub(crate) struct RangeCounts(BTreeMap<u64, (u64, u64)>);

impl RangeCounts {
    /// The parts of `range`, in order, with their counts.
    pub(crate) fn counts(&self, range: Range<u64>) -> Vec<(Range<u64>, u64)> {
        let mut parts = Vec::new();
        let mut offset = range.start;
        for (start, end, count) in self.overlapping(&range) {
            let (start, end) = (start.max(range.start), end.min(range.end));
            if offset < start {
                parts.push((offset..start, 0));
            }
            parts.push((start..end, count));
            offset = end;
        }
        if offset < range.end {
            parts.push((offset..range.end, 0));
        }
        parts
    }

    /// Sets the count of `range`, wholly.
    pub(crate) fn set(&mut self, range: Range<u64>, count: u64) {
        for (start, end, old) in self.overlapping(&range) {
            self.0.remove(&start);
            if start < range.start {
                self.0.insert(start, (range.start, old));
            }
            if end > range.end {
                self.0.insert(range.end, (end, old));
            }
        }
        if count > 0 {
            self.0.insert(range.start, (range.end, count));
        }
    }

    /// Adds one to the count of each part of `range`.
    pub(crate) fn add(&mut self, range: Range<u64>) {
        for (part, count) in self.counts(range) {
            self.set(part, count + 1);
        }
    }

    /// The ranges counted, in order, with their counts.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (Range<u64>, u64)> + '_ {
        self.0.iter().map(|(start, (end, count))| (*start..*end, *count))
    }

    /// The entries overlapping `range`, in order: the one before it may reach into it.
    fn overlapping(&self, range: &Range<u64>) -> Vec<(u64, u64, u64)> {
        let from = self.0.range(..=range.start).next_back().map_or(range.start, |(start, _)| *start);
        self.0
            .range(from..range.end)
            .filter(|(_, (end, _))| *end > range.start)
            .map(|(start, (end, count))| (*start, *end, *count))
            .collect()
    }
}

/// The staging extent, which shared allocations take blocks of (fxdax).
#[derive(Debug)]
struct Staging {
    extent: ExtentId,
    /// The data owner its allocations are recorded under.
    owner_object_id: u64,
    /// Where its next allocation starts.
    next: u64,
}

/// An extent that died, which is freed once `free_after` allows (fxdax).
#[derive(Debug)]
struct DeadExtent {
    free_after: FreeAfter,
    extent: ExtentId,
    size: u64,
    owner_object_id: u64,
}

/// What has to be on the device before a dead extent is freed (fxdax).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FreeAfter {
    /// Nothing: it died in a transaction dropped, which wrote no record of it, or in a
    /// deallocation replay found on the device.
    Now,
    /// The journal up to this offset, where it died, so that a crash can't bring it back. A replay
    /// that verifies the checksums of writes to it also replays its deallocation, which spares
    /// them.
    Flush(u64),
    /// That, and a record of the flush (see `Journal::flush_device`): its owner was deleted, or a
    /// crash left it (an orphan), so that a replay may verify the checksums of writes to it, unless
    /// the record says they are on the device. Offset 0 is any flush.
    RecordedFlush(u64),
}

impl FreeAfter {
    /// Whether the extent can be freed now that the journal is flushed to `log_file_offset`, with
    /// a record of it if `recorded`.
    fn allows(&self, log_file_offset: u64, recorded: bool) -> bool {
        match *self {
            FreeAfter::Now => true,
            FreeAfter::Flush(offset) => offset < log_file_offset,
            FreeAfter::RecordedFlush(offset) => recorded && offset < log_file_offset,
        }
    }
}

struct Inner {
    info: AllocatorInfo,

    /// The allocator can only be opened if there have been no allocations and it has not already
    /// been opened or initialized.
    opened: bool,

    /// The per-owner counters for bytes at various stages of the data life-cycle. From initial
    /// reservation through until the bytes are unallocated.
    owner_bytes: BTreeMap<u64, ByteTracking>,

    /// This value is the number of bytes allocated to reservations but not tracked as part of a
    /// particular volume.
    unattributed_reserved_bytes: u64,

    /// The extents allocated, which are alive.
    extents: HashMap<ExtentId, ExtentInfo>,

    /// The bytes the extents alive take, whole.
    extent_bytes: u64,

    /// The extents that died and are not yet freed, in the order they died.
    dead: VecDeque<DeadExtent>,

    /// The bytes the dead extents take.
    dead_bytes: u64,

    /// The extents freed since the last device flush began, whose frees a crash might undo.
    unsealed: Vec<ExtentId>,

    /// The extents freed before the device flush in progress began, whose frees it seals.
    sealing: Vec<ExtentId>,

    /// The sequence number of the next extent minted.
    next_sequence: u64,

    /// Tracks the number of allocations of size 1,2,...63,>=64.
    allocation_size_histogram: [u64; 64],

    /// fxdax: the staging extent, if there is one.
    staging: Option<Staging>,

    /// fxdax: the extents shared allocations were made in that can't die yet, by how many pins
    /// hold them: one while it is the staging extent, and one for each allocation in it of a
    /// transaction not yet committed.
    shared: HashMap<ExtentId, usize>,
}

impl Inner {
    fn allocated_bytes(&self) -> Saturating<u64> {
        let mut total = Saturating(0);
        for (_, bytes) in &self.owner_bytes {
            total += bytes.allocated_bytes;
        }
        total
    }

    fn uncommitted_allocated_bytes(&self) -> u64 {
        self.owner_bytes.values().map(|x| &x.uncommitted_allocated_bytes).sum()
    }

    fn reserved_bytes(&self) -> u64 {
        self.owner_bytes.values().map(|x| &x.reserved_bytes).sum::<u64>()
            + self.unattributed_reserved_bytes
    }

    fn owner_id_limit_bytes(&self, owner_object_id: u64) -> u64 {
        match self.info.limit_bytes.get(&owner_object_id) {
            Some(v) => *v,
            None => u64::MAX,
        }
    }

    fn owner_id_bytes_left(&self, owner_object_id: u64) -> u64 {
        let limit = self.owner_id_limit_bytes(owner_object_id);
        let used = self
            .owner_bytes
            .get(&self.data_owner(owner_object_id))
            .map_or(Saturating(0), |b| b.used_bytes());
        (Saturating(limit) - used).0
    }

    /// fxdax: the owner that allocations for `owner_object_id`, a store or another owner, are
    /// recorded and counted under: a store's current data owner, which a snapshot changes, and
    /// otherwise the owner itself.
    fn data_owner(&self, owner_object_id: u64) -> u64 {
        *self.info.data_owners.get(&owner_object_id).unwrap_or(&owner_object_id)
    }

    // Returns the amount that is not available to be allocated: the extents alive, whole, those
    // allocated for transactions that haven't committed yet, and those that died but which the
    // device hasn't been flushed for, so can't be freed yet.
    fn unavailable_bytes(&self) -> u64 {
        self.extent_bytes + self.uncommitted_allocated_bytes() + self.dead_bytes
    }

    // Returns the total number of bytes that are taken either from reservations, the extents alive,
    // whole, or uncommitted allocations.
    fn used_bytes(&self) -> u64 {
        self.extent_bytes + self.uncommitted_allocated_bytes() + self.reserved_bytes()
    }

    fn add_reservation(&mut self, owner_object_id: Option<u64>, amount: u64) {
        match owner_object_id {
            Some(owner) => {
                let owner = self.data_owner(owner);
                self.owner_bytes.entry(owner).or_default().reserved_bytes += amount
            }
            None => self.unattributed_reserved_bytes += amount,
        };
    }

    fn remove_reservation(&mut self, owner_object_id: Option<u64>, amount: u64) {
        match owner_object_id {
            Some(owner) => {
                let owner = self.data_owner(owner);
                let owner_entry = self.owner_bytes.entry(owner).or_default();
                assert!(
                    owner_entry.reserved_bytes >= amount,
                    "{} >= {}",
                    owner_entry.reserved_bytes,
                    amount
                );
                owner_entry.reserved_bytes -= amount;
            }
            None => {
                assert!(
                    self.unattributed_reserved_bytes >= amount,
                    "{} >= {}",
                    self.unattributed_reserved_bytes,
                    amount
                );
                self.unattributed_reserved_bytes -= amount
            }
        };
    }

    /// Records an allocation of `range` under `owner_object_id`.
    fn add_allocation(&mut self, range: &ExtentRange, owner_object_id: u64) -> Result<(), Error> {
        let len = range.len();
        self.owner_bytes.entry(owner_object_id).or_default().allocated_bytes += len;
        let extent = self.extents.entry(range.extent).or_insert(ExtentInfo::new(owner_object_id));
        ensure!(
            extent.owner_object_id == owner_object_id,
            anyhow!(FxfsError::Inconsistent).context(format!(
                "{range:?} allocated for {owner_object_id}, but its extent is {}'s",
                extent.owner_object_id
            ))
        );
        extent.allocated += len;
        if range.range.end > extent.size {
            self.extent_bytes += range.range.end - extent.size;
            extent.size = range.range.end;
        }
        // Branch 0's extents are those no allocator mints.
        if range.extent.branch != 0 {
            self.info.next_sequence = self.info.next_sequence.max(range.extent.sequence + 1);
        }
        Ok(())
    }

    /// Records the deallocation of `range`, with the journal at `log_file_offset`, and returns the
    /// extent if it died, with its size and owner. A shared extent that can't die yet stays, with
    /// nothing allocated in it (see "Shared extents").
    fn remove_allocation(
        &mut self,
        range: &ExtentRange,
        log_file_offset: u64,
    ) -> Result<Option<(u64, u64)>, Error> {
        let len = range.len();
        let Some(extent) = self.extents.get_mut(&range.extent) else {
            bail!(anyhow!(FxfsError::Inconsistent).context(format!("{range:?} isn't allocated")));
        };
        let owner_object_id = extent.owner_object_id;
        ensure!(
            range.range.end <= extent.size && extent.allocated >= len,
            anyhow!(FxfsError::Inconsistent)
                .context(format!("{range:?} isn't allocated in {extent:?}"))
        );
        extent.allocated -= len;
        let died = extent.allocated == 0;
        let size = extent.size;
        if let Some(bytes) = self.owner_bytes.get_mut(&owner_object_id) {
            bytes.allocated_bytes -= len;
        }
        if died && self.shared.contains_key(&range.extent) {
            extent.deallocated_at = log_file_offset;
            return Ok(None);
        }
        if !died {
            return Ok(None);
        }
        self.extents.remove(&range.extent);
        self.extent_bytes -= size;
        Ok(Some((size, owner_object_id)))
    }

    /// Kills `extent`, of `size` bytes and `owner_object_id`'s, which is freed once `free_after`
    /// allows.
    fn kill(&mut self, free_after: FreeAfter, extent: ExtentId, size: u64, owner_object_id: u64) {
        self.shared.remove(&extent);
        if self.staging.as_ref().is_some_and(|staging| staging.extent == extent) {
            self.staging = None;
        }
        self.dead_bytes += size;
        self.dead.push_back(DeadExtent { free_after, extent, size, owner_object_id });
    }

    /// The extents dead, and those freed whose frees a crash might undo.
    fn dying(&self) -> Vec<ExtentId> {
        let mut dying: Vec<_> = self
            .dead
            .iter()
            .map(|dead| dead.extent)
            .chain(self.unsealed.iter().copied())
            .chain(self.sealing.iter().copied())
            .collect();
        dying.sort();
        dying.dedup();
        dying
    }

    /// fxdax: mints an extent of `branch`.
    fn mint(&mut self, branch: u64) -> ExtentId {
        let extent = ExtentId::new(branch, self.next_sequence);
        self.next_sequence += 1;
        extent
    }

    /// fxdax: allocates `len` bytes, at most STAGED_MAX, of the staging extent of `owner_object_id`,
    /// a data owner, for a transaction not yet committed, which pins it. Unless that is open on
    /// `device`, with room, a new one of `branch` takes its place, opened on the device.
    fn stage(
        &mut self,
        branch: u64,
        owner_object_id: u64,
        len: u64,
        device: &dyn storage_device::Device,
        tree: &LSMTree<AllocatorKey, AllocatorValue>,
    ) -> ExtentRange {
        debug_assert!(len <= STAGED_MAX);
        let fits = self.staging.as_ref().is_some_and(|staging| {
            staging.owner_object_id == owner_object_id
                && staging.next + len <= STAGING_SIZE
                && device.is_open(staging.extent)
        });
        if !fits {
            self.restage(branch, owner_object_id, device, tree);
        }
        let staging = self.staging.as_mut().unwrap();
        let range = ExtentRange::new(staging.extent, staging.next..staging.next + len);
        staging.next += len;
        *self.shared.get_mut(&staging.extent).unwrap() += 1;
        range
    }

    /// fxdax: makes a new extent of `branch`, opened on `device`, the staging extent of
    /// `owner_object_id`, in place of the last.
    fn restage(
        &mut self,
        branch: u64,
        owner_object_id: u64,
        device: &dyn storage_device::Device,
        tree: &LSMTree<AllocatorKey, AllocatorValue>,
    ) -> ExtentId {
        if let Some(old) = self.staging.take() {
            self.unpin(old.extent, tree);
        }
        let extent = self.mint(branch);
        device.opened(extent);
        self.shared.insert(extent, 1);
        self.staging = Some(Staging { extent, owner_object_id, next: 0 });
        extent
    }

    /// fxdax: drops a pin of the shared `extent`: once nothing pins it, it dies if nothing is
    /// allocated in it, leaving `tree` as a deallocation would. The death isn't journaled: replay
    /// kills it with its last deallocation, which pins don't hold off there.
    fn unpin(&mut self, extent: ExtentId, tree: &LSMTree<AllocatorKey, AllocatorValue>) {
        let Some(pins) = self.shared.get_mut(&extent) else {
            return;
        };
        *pins -= 1;
        if *pins > 0 {
            return;
        }
        self.shared.remove(&extent);
        match self.extents.get(&extent) {
            Some(info) if info.allocated == 0 => {
                let ExtentInfo { size, owner_object_id, deallocated_at, .. } =
                    self.extents.remove(&extent).unwrap();
                self.extent_bytes -= size;
                self.kill(FreeAfter::Flush(deallocated_at), extent, size, owner_object_id);
                let item = AllocatorItem {
                    key: AllocatorKey::new(extent, 0..size),
                    value: AllocatorValue::None,
                };
                let lower_bound = item.key.lower_bound_for_merge_into();
                tree.merge_into(item, &lower_bound);
            }
            Some(_) => {}
            // No transaction that allocated in it committed: its data may have been written.
            None => self.kill(FreeAfter::Now, extent, 0, INVALID_OBJECT_ID),
        }
    }

    /// fxdax: revives `extent`, if it died in this replay but isn't yet freed, which an allocation
    /// replayed is in: as one of a shared extent kept alive, live, by its pins.
    fn revive(&mut self, extent: ExtentId) {
        if self.extents.contains_key(&extent) {
            return;
        }
        if let Some(index) = self.dead.iter().position(|dead| dead.extent == extent) {
            let dead = self.dead.remove(index).unwrap();
            self.dead_bytes -= dead.size;
        }
    }

    /// fxdax: records that `part`, allocated, is referenced `count` times (see "References"), and
    /// returns the record of it.
    fn recount(&mut self, part: ExtentRange, count: u64) -> AllocatorItem {
        let extent = self.extents.get_mut(&part.extent).expect("allocated");
        extent.references.set(part.range.clone(), if count > 1 { count } else { 0 });
        AllocatorItem {
            key: AllocatorKey::new(part.extent, part.range),
            value: AllocatorValue::Abs { count, owner_object_id: extent.owner_object_id },
        }
    }

    /// Kills the extents of `owners`, which are deleted.
    fn kill_owners(&mut self, owners: &[u64], free_after: FreeAfter) {
        let extents: Vec<_> = self
            .extents
            .iter()
            .filter(|(_, info)| owners.contains(&info.owner_object_id))
            .map(|(extent, info)| (*extent, info.size, info.owner_object_id))
            .collect();
        for (extent, size, owner_object_id) in extents {
            self.extents.remove(&extent);
            self.extent_bytes -= size;
            self.kill(free_after, extent, size, owner_object_id);
        }
    }
}

impl Allocator {
    pub fn new(filesystem: Arc<FxFilesystem>, object_id: u64) -> Allocator {
        let block_size = filesystem.block_size();
        // We expect device size to be a multiple of block size. Throw away any tail.
        let device_size = block_size.align_down(filesystem.device().size());
        if device_size != filesystem.device().size() {
            warn!("Device size is not block aligned. Rounding down.");
        }
        let max_extent_size_bytes = max_extent_size_for_block_size(block_size);
        Allocator {
            filesystem: Arc::downgrade(&filesystem),
            block_size,
            device_size: AtomicU64::new(device_size),
            branch: AtomicU64::new(filesystem.device().branch()),
            object_id,
            max_extent_size_bytes,
            tree: LSMTree::new(merge, None),
            inner: Mutex::new(Inner {
                info: AllocatorInfo::default(),
                opened: false,
                owner_bytes: BTreeMap::new(),
                unattributed_reserved_bytes: 0,
                extents: HashMap::new(),
                extent_bytes: 0,
                dead: VecDeque::new(),
                dead_bytes: 0,
                unsealed: Vec::new(),
                sealing: Vec::new(),
                next_sequence: 0,
                allocation_size_histogram: [0; 64],
                staging: None,
                shared: HashMap::new(),
            }),
            counters: Mutex::new(AllocatorCounters::default()),
            allocations_allowed: AtomicBool::new(filesystem.options().image_builder_mode.is_none()),
            freeing: futures::lock::Mutex::new(None),
        }
    }

    pub fn tree(&self) -> &LSMTree<AllocatorKey, AllocatorValue> {
        &self.tree
    }

    /// Enables allocations.
    /// This is only valid to call if the allocator was created in image builder mode.
    pub fn enable_allocations(&self) {
        self.allocations_allowed.store(true, Ordering::SeqCst);
    }

    /// Returns an iterator that yields all allocations, filtering out tombstones and any
    /// owner_object_id that have been marked as deleted. fxdax: dead ranges of extents still alive
    /// (count 0) are yielded too.
    pub async fn filter<'a>(
        &self,
        iter: impl LayerIterator<AllocatorKey, AllocatorValue> + 'a,
    ) -> Result<impl LayerIterator<AllocatorKey, AllocatorValue> + 'a, Error> {
        let marked_for_deletion = self.inner.lock().info.marked_for_deletion.clone();
        let iter =
            filter_marked_for_deletion(filter_tombstones(iter).await?, marked_for_deletion).await?;
        Ok(iter)
    }

    /// A histogram of allocation request sizes.
    /// The index into the array is 'number of blocks'.
    /// The last bucket is a catch-all for larger allocation requests.
    pub fn allocation_size_histogram(&self) -> [u64; 64] {
        self.inner.lock().allocation_size_histogram
    }

    /// Creates a new (empty) allocator.
    pub async fn create(&self, transaction: &mut Transaction<'_>) -> Result<(), Error> {
        // Mark the allocator as opened before creating the file because creating a new
        // transaction requires a reservation.
        assert_eq!(std::mem::replace(&mut self.inner.lock().opened, true), false);

        let filesystem = self.filesystem.upgrade().unwrap();
        // Minting starts past any extent of the branch the device holds already, and past its
        // floor.
        let device = filesystem.device();
        let next_sequence = device.next_sequence().await?.max(device.sequence_floor().await?);
        self.inner.lock().next_sequence = next_sequence;
        let root_store = filesystem.root_store();
        ObjectStore::create_object_with_id(
            &root_store,
            transaction,
            ReservedId::new(&root_store, NonZero::new(self.object_id()).unwrap()),
            HandleOptions::default(),
            None,
        )?;
        root_store.update_last_object_id(self.object_id());
        Ok(())
    }

    // Opens the allocator.  This is not thread-safe; this should be called prior to
    // replaying the allocator's mutations.
    pub async fn open(self: &Arc<Self>) -> Result<(), Error> {
        let filesystem = self.filesystem.upgrade().unwrap();
        let root_store = filesystem.root_store();

        let handle =
            ObjectStore::open_object(&root_store, self.object_id, HandleOptions::default(), None)
                .await
                .context("Failed to open allocator object")?;

        if handle.get_size() > 0 {
            let serialized_info = handle
                .contents(handle.get_size() as usize)
                .await
                .context("Failed to read AllocatorInfo")?;
            let mut cursor = std::io::Cursor::new(serialized_info);
            let (info, _version) = AllocatorInfo::deserialize_with_version(&mut cursor)
                .context("Failed to deserialize AllocatorInfo")?;

            let (layers, total_size) = open_layers(&root_store, info.layers.iter().cloned(), None)
                .await
                .context("Failed to open allocator layer file")?;

            {
                let mut inner = self.inner.lock();

                // Check allocated_bytes fits within the device.
                let mut device_bytes = self.device_size();
                for (&owner_object_id, &bytes) in &info.allocated_bytes {
                    ensure!(
                        bytes <= device_bytes,
                        anyhow!(FxfsError::Inconsistent).context(format!(
                            "Allocated bytes exceeds device size: {:?}",
                            info.allocated_bytes
                        ))
                    );
                    device_bytes -= bytes;

                    inner.owner_bytes.entry(owner_object_id).or_default().allocated_bytes =
                        Saturating(bytes);
                }

                inner.info = info;
            }

            self.tree.append_open_layers(layers);
            self.filesystem.upgrade().unwrap().object_manager().update_reservation(
                self.object_id,
                tree::reservation_amount_from_layer_size(total_size),
            );

            self.load_extents().await.context("Failed to load extents")?;
        }

        Ok(())
    }

    /// fxdax: finds the extents the tree holds, before replay: those alive, and those of owners
    /// marked for deletion, which are dead.
    async fn load_extents(&self) -> Result<(), Error> {
        let marked_for_deletion = self.inner.lock().info.marked_for_deletion.clone();
        let mut extents = HashMap::<ExtentId, ExtentInfo>::new();
        let layer_set = self.tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = filter_tombstones(merger.query(Query::FullScan).await?).await?;
        while let Some(ItemRef { key, value, .. }) = iter.get() {
            let AllocatorValue::Abs { count, owner_object_id } = *value else {
                unreachable!("filtered out");
            };
            let extent = extents.entry(key.extent).or_insert(ExtentInfo::new(owner_object_id));
            ensure!(
                extent.owner_object_id == owner_object_id,
                anyhow!(FxfsError::Inconsistent).context(format!(
                    "{key:?} is {owner_object_id}'s, and its extent {}'s",
                    extent.owner_object_id
                ))
            );
            extent.size = extent.size.max(key.range.end);
            if count > 0 {
                extent.allocated += key.range.length()?;
            }
            if count > 1 {
                extent.references.set(key.range.0.clone(), count);
            }
            iter.advance().await?;
        }
        let mut inner = self.inner.lock();
        // The extents dying when the tree was last compacted, which it doesn't hold, are freed again.
        for extent in std::mem::take(&mut inner.info.dying) {
            if !extents.contains_key(&extent) {
                inner.kill(FreeAfter::Now, extent, 0, INVALID_OBJECT_ID);
            }
        }
        for (extent, info) in extents {
            if marked_for_deletion.contains(&info.owner_object_id) {
                inner.kill(FreeAfter::RecordedFlush(0), extent, info.size, info.owner_object_id);
            } else if info.allocated == 0 {
                inner.kill(FreeAfter::Now, extent, info.size, info.owner_object_id);
            } else {
                inner.extent_bytes += info.size;
                inner.extents.insert(extent, info);
            }
        }
        Ok(())
    }

    pub async fn on_replay_complete(self: &Arc<Self>) -> Result<(), Error> {
        let filesystem = self.filesystem.upgrade().unwrap();
        // Minting resumes past every extent of the branch the device holds, including those a
        // crash left, whose transactions were lost: orphans, which die. They are freed with the
        // next flush, once the journal records what replay discarded, and the flush (see
        // `Journal::flush_device`): replay verifies the checksums of writes to them otherwise.
        // fxdax: and past the device's floor, another writer's range's start, which no extent the
        // device holds need reach: orphans are looked for below what it holds only.
        let device_next_sequence = filesystem.device().next_sequence().await?;
        let floor = filesystem.device().sequence_floor().await?;
        {
            let mut inner = self.inner.lock();
            let next_sequence = inner.info.next_sequence;
            inner.next_sequence = next_sequence.max(device_next_sequence).max(floor);
            let orphans: Vec<_> = (next_sequence..device_next_sequence)
                .map(|sequence| ExtentId::new(self.branch(), sequence))
                .filter(|extent| !inner.extents.contains_key(extent))
                .collect();
            if !orphans.is_empty() {
                info!(count = orphans.len(); "Extents a crash left will be freed");
            }
            for orphan in orphans {
                inner.kill(FreeAfter::RecordedFlush(0), orphan, 0, INVALID_OBJECT_ID);
            }
            assert_eq!(std::mem::replace(&mut inner.opened, true), false);
        }
        // The extents whose deallocation replay found are freed now.
        if !filesystem.options().read_only {
            self.free_dead(&*filesystem.device(), 0, false)
                .await
                .context("Failed to free dead extents")?;
        }
        Ok(())
    }

    /// Returns all objects that exist in the parent store that pertain to this allocator.
    pub fn parent_objects(&self) -> Vec<u64> {
        // The allocator tree needs to store a file for each of the layers in the tree, so we return
        // those, since nothing else references them.
        self.inner.lock().info.layers.clone()
    }

    /// Returns all the current owner byte limits (in pairs of `(owner_object_id, bytes)`).
    pub fn owner_byte_limits(&self) -> Vec<(u64, u64)> {
        self.inner.lock().info.limit_bytes.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// Returns (allocated_bytes, byte_limit) for the given owner.
    pub fn owner_allocation_info(&self, owner_object_id: u64) -> (u64, Option<u64>) {
        let inner = self.inner.lock();
        (
            inner.owner_bytes.get(&owner_object_id).map(|b| b.used_bytes().0).unwrap_or(0u64),
            inner.info.limit_bytes.get(&owner_object_id).copied(),
        )
    }

    /// Returns owner bytes debug information.
    pub fn owner_bytes_debug(&self) -> String {
        format!("{:?}", self.inner.lock().owner_bytes)
    }

    /// Whether allocating `len` bytes needs the device flushed first, so that dead extents can be
    /// freed to make room.
    fn needs_sync(&self, len: u64) -> bool {
        let inner = self.inner.lock();
        inner.dead_bytes > 0 && inner.unavailable_bytes() + len > self.device_size()
    }

    fn is_system_store(&self, owner_object_id: u64) -> bool {
        let fs = self.filesystem.upgrade().unwrap();
        owner_object_id == fs.object_manager().root_store_object_id()
            || owner_object_id == fs.object_manager().root_parent_store_object_id()
    }

    /// Updates the accounting to track that a byte reservation has been moved out of an owner to
    /// the unattributed pool.
    pub fn disown_reservation(&self, old_owner_object_id: Option<u64>, amount: u64) {
        if old_owner_object_id.is_none() || amount == 0 {
            return;
        }
        // These 2 mutations should behave as though they're a single atomic mutation.
        let mut inner = self.inner.lock();
        inner.remove_reservation(old_owner_object_id, amount);
        inner.add_reservation(None, amount);
    }

    /// Creates a lazy inspect node named `str` under `parent` which will yield statistics for the
    /// allocator when queried.
    pub fn track_statistics(self: &Arc<Self>, parent: &fuchsia_inspect::Node, name: &str) {
        let this = Arc::downgrade(self);
        parent.record_lazy_child(name, move || {
            let this_clone = this.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                if let Some(this) = this_clone.upgrade() {
                    let counters = this.counters.lock();
                    let root = inspector.root();
                    root.record_uint("max_extent_size_bytes", this.max_extent_size_bytes);
                    root.record_uint("bytes_total", this.device_size());
                    let (allocated, reserved, used, unavailable, extents, dead) = {
                        // TODO(https://fxbug.dev/42069513): Push-back or rate-limit to prevent DoS.
                        let inner = this.inner.lock();
                        (
                            inner.allocated_bytes().0,
                            inner.reserved_bytes(),
                            inner.used_bytes(),
                            inner.unavailable_bytes(),
                            inner.extents.len() as u64,
                            inner.dead.len() as u64,
                        )
                    };
                    root.record_uint("bytes_allocated", allocated);
                    root.record_uint("bytes_reserved", reserved);
                    root.record_uint("bytes_used", used);
                    root.record_uint("bytes_unavailable", unavailable);
                    root.record_uint("extents", extents);
                    root.record_uint("dead_extents", dead);

                    // TODO(https://fxbug.dev/42068224): Post-compute rather than manually computing
                    // metrics.
                    if let Some(x) = round_div(100 * allocated, this.device_size()) {
                        root.record_uint("bytes_allocated_percent", x);
                    }
                    if let Some(x) = round_div(100 * reserved, this.device_size()) {
                        root.record_uint("bytes_reserved_percent", x);
                    }
                    if let Some(x) = round_div(100 * used, this.device_size()) {
                        root.record_uint("bytes_used_percent", x);
                    }
                    if let Some(x) = round_div(100 * unavailable, this.device_size()) {
                        root.record_uint("bytes_unavailable_percent", x);
                    }

                    root.record_uint("num_flushes", counters.num_flushes);
                    if let Some(last_flush_time) = counters.last_flush_time.as_ref() {
                        root.record_uint(
                            "last_flush_time_ms",
                            last_flush_time
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or(std::time::Duration::ZERO)
                                .as_millis()
                                .try_into()
                                .unwrap_or(0u64),
                        );
                    }

                    let data = this.allocation_size_histogram();
                    let alloc_sizes = root.create_uint_linear_histogram(
                        "allocation_size_histogram",
                        fuchsia_inspect::LinearHistogramParams {
                            floor: 1,
                            step_size: 1,
                            buckets: 64,
                        },
                    );
                    for (i, count) in data.iter().enumerate() {
                        if i != 0 {
                            alloc_sizes.insert_multiple(i as u64, *count as usize);
                        }
                    }
                    root.record(alloc_sizes);

                    let allocator_ref = this.clone();
                    root.record_child("lsm_tree", move |node| {
                        allocator_ref.tree.record_inspect_data(node);
                    });
                }
                Ok(inspector)
            }
            .boxed()
        });
    }
}

impl Drop for Allocator {
    fn drop(&mut self) {
        let inner = self.inner.lock();
        // Uncommitted and reserved should be released back using RAII, so they should be zero.
        assert_eq!(inner.uncommitted_allocated_bytes(), 0);
        assert_eq!(inner.reserved_bytes(), 0);
    }
}

#[fxfs_trace::trace]
impl Allocator {
    /// Returns the object ID for the allocator.
    pub fn object_id(&self) -> u64 {
        self.object_id
    }

    /// Returns information about the allocator such as the layer files storing persisted
    /// allocations.
    pub fn info(&self) -> AllocatorInfo {
        self.inner.lock().info.clone()
    }

    fn reservation_description(&self, reservation: &Reservation) -> String {
        let metadata_info = self.filesystem.upgrade().and_then(|fs| {
            let om = fs.object_manager();
            if om.is_metadata_reservation(reservation) {
                Some((om.borrowed_metadata_space(), om.max_store_reservation()))
            } else {
                None
            }
        });
        match metadata_info {
            Some((borrowed, max_store_reservation)) => {
                format!(
                    "reservation with {} bytes available (borrowed metadata space: {}, max store \
                    reservation: {})",
                    reservation.amount(),
                    borrowed,
                    max_store_reservation
                )
            }
            None => {
                format!("reservation with {} bytes available", reservation.amount())
            }
        }
    }

    /// Tries to allocate enough space for |object_range| in the specified object and returns the
    /// extent allocated, a new one, whose range is from 0.
    /// The allocated range may be short (e.g. due to the reservation), in which case the caller can
    /// simply call allocate again until they have enough blocks.
    ///
    /// We also store the object store ID of the store that the allocation should be assigned to so
    /// that we have a means to delete encrypted stores without needing the encryption key.
    #[trace]
    pub async fn allocate(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        len: u64,
    ) -> Result<ExtentRange, Error> {
        self.allocate_in(transaction, owner_object_id, len, false).await
    }

    /// fxdax: as `allocate`, but of at most STAGED_MAX bytes, which share the staging extent with
    /// other allocations (see "Shared extents"): open until the device's next barrier, so that
    /// they can be written until then.
    pub async fn allocate_shared(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        len: u64,
    ) -> Result<ExtentRange, Error> {
        self.allocate_in(transaction, owner_object_id, len.min(STAGED_MAX), true).await
    }

    /// fxdax: replaces the staging extent, if a barrier sealed it since blocks were taken of it,
    /// with a new one, opened on the device, which the next shared allocations take blocks of:
    /// returned, so that a guest's DAX window can map it before a write waits for that. One left
    /// unused is sealed by the next barrier, and dies, as the staging extent no more, then.
    pub fn restage(&self) -> Option<ExtentId> {
        let device = self.filesystem.upgrade()?.device();
        let mut inner = self.inner.lock();
        let staging = inner.staging.as_ref()?;
        if staging.next == 0 || device.is_open(staging.extent) {
            return None;
        }
        let owner_object_id = staging.owner_object_id;
        Some(inner.restage(self.branch(), owner_object_id, &*device, &self.tree))
    }

    /// fxdax: whether `extent` is shared, and alive whatever is allocated in it: the staging
    /// extent, or one a transaction not yet committed has allocated in (see "Shared extents").
    pub fn is_held(&self, extent: ExtentId) -> bool {
        self.inner.lock().shared.contains_key(&extent)
    }

    /// fxdax: how far `extent` reaches, as allocated, or for the staging extent, while it is open,
    /// may yet be.
    pub fn extent_size(&self, extent: ExtentId) -> Option<u64> {
        let device = self.filesystem.upgrade().unwrap().device();
        let inner = self.inner.lock();
        if inner.staging.as_ref().is_some_and(|staging| staging.extent == extent)
            && device.is_open(extent)
        {
            return Some(STAGING_SIZE);
        }
        inner.extents.get(&extent).map(|info| info.size)
    }

    async fn allocate_in(
        self: &Arc<Self>,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        mut len: u64,
        shared: bool,
    ) -> Result<ExtentRange, Error> {
        ensure!(self.allocations_allowed.load(Ordering::SeqCst), FxfsError::Unavailable);
        assert!(self.block_size.is_aligned(len));
        len = std::cmp::min(len, self.max_extent_size_bytes);
        debug_assert_ne!(owner_object_id, INVALID_OBJECT_ID);

        let requested_len = len;
        // Make sure we have space reserved before we try and find the space.
        let reservation = if let Some(reservation) = transaction.allocator_reservation {
            match reservation.owner_object_id {
                // If there is no owner, this must be a system store that we're allocating for.
                None => assert!(self.is_system_store(owner_object_id)),
                // If it has an owner, it should not be different than the allocating owner.
                Some(res_owner_object_id) => assert_eq!(owner_object_id, res_owner_object_id),
            };
            // Reservation limits aren't necessarily a multiple of the block size.
            let r = reservation
                .reserve_with(|limit| std::cmp::min(len, self.block_size.align_down(limit)));
            len = r.amount();
            Left(r)
        } else {
            let mut inner = self.inner.lock();
            assert!(inner.opened);
            // Do not exceed the limit for the owner or the device.
            let device_used = inner.used_bytes();
            let owner_bytes_left = inner.owner_id_bytes_left(owner_object_id);
            // We must take care not to use up space that might be reserved.
            let limit =
                std::cmp::min(owner_bytes_left, self.device_size().saturating_sub(device_used));
            len = self.block_size.align_down(std::cmp::min(len, limit));
            let data_owner = inner.data_owner(owner_object_id);
            let owner_entry = inner.owner_bytes.entry(data_owner).or_default();
            owner_entry.reserved_bytes += len;
            Right(ReservationImpl::<_, Self>::new(&**self, Some(owner_object_id), len))
        };

        if len == 0 {
            if let Some(reservation) = transaction.allocator_reservation {
                bail!(anyhow!(FxfsError::NoSpace).context(format!(
                    "Failed to allocate {} bytes for owner {} from {}",
                    requested_len,
                    owner_object_id,
                    self.reservation_description(reservation),
                )));
            } else {
                let inner = self.inner.lock();
                bail!(anyhow!(FxfsError::NoSpace).context(format!(
                    "Failed to allocate {} bytes for owner {} (owner_bytes_left: {}, device_used: \
                    {}, device_size: {})",
                    requested_len,
                    owner_object_id,
                    inner.owner_id_bytes_left(owner_object_id),
                    inner.used_bytes(),
                    self.device_size()
                )));
            }
        }

        // If dead extents take the room the allocation needs, sync and flush the device, so that
        // they are freed.
        //
        // We can't hold the allocator's lock whilst we sync here because apply_mutation takes it,
        // when journal locks are held, and sync takes those same locks. Sync holds its own lock to
        // guard against multiple syncs occurring at the same time, and we can supply a precondition
        // that is evaluated under that lock to ensure we don't sync twice if we don't need to.
        let mut syncs = 0;
        while self.needs_sync(len) {
            // Cap number of sync attempts before giving up on finding free space.
            ensure!(
                syncs < 10,
                anyhow!(FxfsError::NoSpace).context("Sync failed to yield sufficient free space.")
            );
            syncs += 1;
            self.filesystem
                .upgrade()
                .unwrap()
                .sync(SyncOptions {
                    flush_device: true,
                    precondition: Some(Box::new(|| self.needs_sync(len))),
                    ..Default::default()
                })
                .await?;
        }

        let reservation_owner = reservation.either(
            // Left means we got an outside reservation.
            |l| {
                l.forget_some(len);
                l.owner_object_id()
            },
            |r| {
                r.forget_some(len);
                r.owner_object_id()
            },
        );

        let device = self.filesystem.upgrade().unwrap().device();
        let result = {
            let mut inner = self.inner.lock();
            inner.allocation_size_histogram[std::cmp::min(63, len / self.block_size) as usize] += 1;
            let data_owner = inner.data_owner(owner_object_id);
            let result = if shared {
                inner.stage(self.branch(), data_owner, len, &*device, &self.tree)
            } else {
                // Open until a seal: none can have sealed an extent not yet minted.
                let extent = inner.mint(self.branch());
                device.opened(extent);
                ExtentRange::new(extent, 0..len)
            };
            let owner_entry = inner.owner_bytes.entry(data_owner).or_default();
            owner_entry.uncommitted_allocated_bytes += len;
            // If the reservation has an owner, ensure they are the same.
            assert_eq!(owner_object_id, reservation_owner.unwrap_or(owner_object_id));
            inner.remove_reservation(reservation_owner, len);
            result
        };

        debug!(device_range:? = result; "allocate");
        let mutation =
            AllocatorMutation::Allocate { device_range: result.clone(), owner_object_id };
        assert!(transaction.add(self.object_id(), Mutation::Allocator(mutation)).is_none());

        Ok(result)
    }

    /// Marks the given range as allocated, in an extent no allocator mints.  The main use case
    /// for this at this time is for the super-block which needs to be at a fixed location.
    #[trace]
    pub fn mark_allocated(
        &self,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        device_range: ExtentRange,
    ) -> Result<(), Error> {
        debug_assert_ne!(owner_object_id, INVALID_OBJECT_ID);
        {
            let len = device_range.range.length().map_err(|_| FxfsError::InvalidArgs)?;

            let mut inner = self.inner.lock();
            let device_used = inner.used_bytes();
            let owner_id_bytes_left = inner.owner_id_bytes_left(owner_object_id);
            let data_owner = inner.data_owner(owner_object_id);
            ensure!(
                device_range.extent.branch == 0
                    && !inner.extents.contains_key(&device_range.extent)
                    && self.device_size().saturating_sub(device_used) >= len
                    && owner_id_bytes_left >= len,
                anyhow!(FxfsError::NoSpace).context(format!(
                    "Failed to allocate {} bytes at {:?} for owner {} (owner_bytes_left: {}, \
                    device_used: {}, device_size: {})",
                    len,
                    device_range,
                    owner_object_id,
                    owner_id_bytes_left,
                    device_used,
                    self.device_size()
                ))
            );
            if let Some(reservation) = &mut transaction.allocator_reservation {
                // The transaction takes ownership of this hold.
                reservation
                    .reserve(len)
                    .ok_or_else(|| {
                        anyhow!(FxfsError::NoSpace).context(format!(
                            "Failed to reserve {} bytes at {:?} from {}",
                            len,
                            device_range,
                            self.reservation_description(reservation),
                        ))
                    })?
                    .forget();
            }
            inner.owner_bytes.entry(data_owner).or_default().uncommitted_allocated_bytes += len;
        }
        let mutation = AllocatorMutation::Allocate { device_range, owner_object_id };
        transaction.add(self.object_id(), Mutation::Allocator(mutation));
        Ok(())
    }

    /// Sets the limits for an owner object in terms of usage.
    pub fn set_bytes_limit(
        &self,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        bytes: u64,
    ) -> Result<(), Error> {
        // System stores cannot be given limits.
        assert!(!self.is_system_store(owner_object_id));
        transaction.add(
            self.object_id(),
            Mutation::Allocator(AllocatorMutation::SetLimit { owner_object_id, bytes }),
        );
        Ok(())
    }

    /// Gets the bytes limit for an owner object.
    pub fn get_owner_bytes_limit(&self, owner_object_id: u64) -> Option<u64> {
        self.inner.lock().info.limit_bytes.get(&owner_object_id).copied()
    }

    /// Gets the number of bytes currently used by allocations, reservations or uncommitted
    /// allocations.
    pub fn get_owner_bytes_used(&self, owner_object_id: u64) -> u64 {
        let inner = self.inner.lock();
        let owner = inner.data_owner(owner_object_id);
        inner.owner_bytes.get(&owner).map_or(0, |info| info.used_bytes().0)
    }

    /// Deallocates the given range for the specified object. Its extent dies once the last of it is
    /// deallocated.
    #[trace]
    pub async fn deallocate(
        &self,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        dealloc_range: ExtentRange,
    ) -> Result<u64, Error> {
        debug!(device_range:? = dealloc_range; "deallocate");
        ensure!(dealloc_range.range.is_valid(), FxfsError::InvalidArgs);
        // fxdax: the extents frozen owners hold are shared by stores a snapshot made (see
        // `snapshot`), and stay allocated; only the rest are the store's to deallocate. Of those,
        // the parts referenced more than once lose a reference (see "References").
        let deallocated = dealloc_range.len();
        if self.frozen_owner(&dealloc_range).is_none() {
            let mutation = AllocatorMutation::Deallocate {
                device_range: dealloc_range.clone(),
                owner_object_id,
            };
            // A range a file's extents reference twice (a reflink within it) may be deallocated
            // twice in a transaction, which holds the mutation once: the second is a reference
            // fewer.
            if transaction.add(self.object_id(), Mutation::Allocator(mutation)).is_some() {
                transaction.add_reference(self.object_id(), dealloc_range, owner_object_id, -1);
            }
        }
        Ok(deallocated)
    }

    /// fxdax: the frozen owner holding `range`'s extent, if one does.
    fn frozen_owner(&self, range: &ExtentRange) -> Option<u64> {
        let inner = self.inner.lock();
        let owner = inner.extents.get(&range.extent)?.owner_object_id;
        inner.info.frozen_owners.contains_key(&owner).then_some(owner)
    }

    /// fxdax: the parts of `range` that frozen owners hold, in order, with their owners. Their
    /// data does not change while their owners are frozen.
    pub async fn frozen_ranges(
        &self,
        range: ExtentRange,
    ) -> Result<Vec<(ExtentRange, u64)>, Error> {
        Ok(self.frozen_owner(&range).map(|owner| (range, owner)).into_iter().collect())
    }

    /// fxdax: whether `range` is shared, and must not be overwritten in place: its extent is a
    /// frozen owner's, shared with other stores, or some of it is referenced more than once
    /// (see "References").
    pub async fn is_shared(&self, range: ExtentRange) -> Result<bool, Error> {
        if self.frozen_owner(&range).is_some() {
            return Ok(true);
        }
        let inner = self.inner.lock();
        Ok(inner.extents.get(&range.extent).is_some_and(|extent| {
            extent.references(range.range.clone()).iter().any(|(_, count)| *count > 1)
        }))
    }

    /// fxdax: references `range`, which `owner_object_id`'s store has allocated, once more: for
    /// another extent of a file to share it (a reflink, `DataObjectHandle::txn_clone_range`). A
    /// frozen owner's range is shared already, and stays allocated while the store does.
    pub fn reference(
        &self,
        transaction: &mut Transaction<'_>,
        owner_object_id: u64,
        range: ExtentRange,
    ) {
        if self.frozen_owner(&range).is_none() {
            transaction.add_reference(self.object_id(), range, owner_object_id, 1);
        }
    }

    /// fxdax: snapshots the store `source_store_object_id` into the new store
    /// `new_store_object_id`, which shares all of the source's extents. The source's current data
    /// owner is frozen: both stores share its extents, which neither deallocates, until no store
    /// shares them. From then on, the source allocates under `new_owner_object_id`, which must be
    /// an object id never used as an owner.
    pub fn snapshot(
        &self,
        transaction: &mut Transaction<'_>,
        source_store_object_id: u64,
        new_store_object_id: u64,
        new_owner_object_id: u64,
    ) {
        transaction.add(
            self.object_id(),
            Mutation::Allocator(AllocatorMutation::Snapshot {
                source_store_object_id,
                new_store_object_id,
                new_owner_object_id,
            }),
        );
    }

    /// fxdax: the owner the store `store_object_id` allocates under.
    pub fn data_owner(&self, store_object_id: u64) -> u64 {
        self.inner.lock().data_owner(store_object_id)
    }

    /// fxdax: the frozen owners whose extents the store `store_object_id` shares.
    pub fn shared_owners(&self, store_object_id: u64) -> Vec<u64> {
        self.inner.lock().info.shared_owners.get(&store_object_id).cloned().unwrap_or_default()
    }

    /// fxdax: whether any owner is frozen.
    pub fn has_frozen_owners(&self) -> bool {
        !self.inner.lock().info.frozen_owners.is_empty()
    }

    /// fxdax: the frozen owners, with the number of stores sharing their extents.
    pub fn frozen_owners(&self) -> BTreeMap<u64, u64> {
        self.inner.lock().info.frozen_owners.clone()
    }

    /// fxdax: the current data owners of the stores snapshots gave new ones, by store object id.
    pub fn data_owners(&self) -> BTreeMap<u64, u64> {
        self.inner.lock().info.data_owners.clone()
    }

    /// fxdax: the extents alive, with their owners and the bytes of them allocated, for fsck.
    pub fn extents(&self) -> HashMap<ExtentId, (u64, u64)> {
        let inner = self.inner.lock();
        inner
            .extents
            .iter()
            .map(|(extent, info)| (*extent, (info.owner_object_id, info.allocated)))
            .collect()
    }

    /// fxdax: the extents that died and are not yet freed.
    pub fn dead_extents(&self) -> Vec<ExtentId> {
        self.inner.lock().dead.iter().map(|dead| dead.extent).collect()
    }

    /// Marks allocations associated with a given |owner_object_id| for deletion.
    ///
    /// This is used as part of deleting encrypted volumes (ObjectStore) without having the keys.
    ///
    /// MarkForDeletion mutations eventually manipulates allocator metadata (AllocatorInfo) instead
    /// of the mutable layer but we must be careful not to do this too early and risk premature
    /// freeing of extents.
    ///
    /// Replay is not guaranteed until the *device* gets flushed, so we cannot free the deleted
    /// extents until we've flushed the device.
    ///
    /// TODO(b/316827348): Consider removing the use of mark_for_deletion in AllocatorInfo and
    /// just compacting?
    ///
    /// After an allocator.flush() (i.e. a major compaction), we know that there is no data left
    /// in the layer files for this owner_object_id and we are able to clear `marked_for_deletion`.
    pub fn mark_for_deletion(&self, transaction: &mut Transaction<'_>, owner_object_id: u64) {
        // Note that because the actual time of deletion (the next major compaction) is undefined,
        // |owner_object_id| should not be reused after this call.
        transaction.add(
            self.object_id(),
            Mutation::Allocator(AllocatorMutation::MarkForDeletion(owner_object_id)),
        );
    }

    /// fxdax: whether freeing the extents that died before `log_file_offset` needs a record of
    /// the device's flush to it (see `FreeAfter::RecordedFlush`).
    pub fn needs_recorded_flush(&self, log_file_offset: u64) -> bool {
        let inner = self.inner.lock();
        inner.dead.iter().any(|dead| match dead.free_after {
            FreeAfter::RecordedFlush(offset) => offset < log_file_offset,
            _ => false,
        })
    }

    /// Called when the device has been flush and indicates what the journal log offset was when
    /// that happened, and whether the journal records that. The extents that died before then are
    /// freed.
    ///
    /// fxdax: freed in the background, as `freed` waits for: each free is a round trip to the
    /// device, which a sync need not wait for (`SyncOptions::background_frees`).
    pub async fn did_flush_device(self: &Arc<Self>, flush_log_offset: u64, recorded: bool) {
        self.inner.lock().sealing.clear();
        let device = self.filesystem.upgrade().unwrap().device();
        let this = self.clone();
        let mut freeing = self.freeing.lock().await;
        let before = freeing.take();
        *freeing = Some(fasync::Task::spawn(async move {
            let before = match before {
                Some(before) => before.await,
                None => Ok(()),
            };
            let freed = this.free_dead(&*device, flush_log_offset, recorded).await;
            before.and(freed)
        }));
    }

    /// fxdax: waits for the frees of the extents that the device flushes so far let die
    /// (`did_flush_device`).
    pub async fn freed(&self) -> Result<(), Error> {
        let mut freeing = self.freeing.lock().await;
        match freeing.as_mut() {
            Some(task) => {
                let freed = task.await;
                *freeing = None;
                freed
            }
            None => Ok(()),
        }
    }

    /// fxdax: called as the device begins a flush, which seals the frees issued before it.
    pub fn will_flush_device(&self) {
        let mut inner = self.inner.lock();
        let unsealed = std::mem::take(&mut inner.unsealed);
        inner.sealing.extend(unsealed);
    }

    /// Frees the dead extents that a flush of the journal to `log_file_offset`, recorded if
    /// `recorded`, allows.
    async fn free_dead(
        &self,
        device: &dyn storage_device::Device,
        log_file_offset: u64,
        recorded: bool,
    ) -> Result<(), Error> {
        let dead: Vec<_> = {
            let mut inner = self.inner.lock();
            let (free, wait): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut inner.dead)
                .into_iter()
                .partition(|dead| dead.free_after.allows(log_file_offset, recorded));
            inner.dead = wait;
            free.into()
        };
        if dead.is_empty() {
            return Ok(());
        }
        let extents: Vec<_> = dead.iter().map(|dead| dead.extent).collect();
        let results = device.free_extents(&extents).await;
        let mut inner = self.inner.lock();
        let mut first_error = None;
        // The extents not freed are tried again later.
        for (dead, result) in dead.into_iter().zip(results) {
            match result {
                Ok(()) => {
                    inner.dead_bytes -= dead.size;
                    inner.unsealed.push(dead.extent);
                }
                Err(error) => {
                    error!(error:?, extent:? = dead.extent; "Failed to free extent");
                    first_error = Some(error);
                    inner.dead.push_back(dead);
                }
            }
        }
        match first_error {
            Some(error) => Err(error.context("Failed to free dead extents")),
            None => Ok(()),
        }
    }

    /// Returns a reservation that can be used later, or None if there is insufficient space. The
    /// |owner_object_id| indicates which object in the root object store the reservation is for.
    pub fn reserve(
        self: Arc<Self>,
        owner_object_id: Option<u64>,
        amount: u64,
    ) -> Option<Reservation> {
        {
            let mut inner = self.inner.lock();

            let device_free = self.device_size().saturating_sub(inner.used_bytes());

            let limit = match owner_object_id {
                Some(id) => std::cmp::min(inner.owner_id_bytes_left(id), device_free),
                None => device_free,
            };
            if limit < amount {
                return None;
            }
            inner.add_reservation(owner_object_id, amount);
        }
        Some(Reservation::new(self, owner_object_id, amount))
    }

    /// Like reserve, but takes a callback is passed the |limit| and should return the amount,
    /// which can be zero.
    pub fn reserve_with(
        self: Arc<Self>,
        owner_object_id: Option<u64>,
        amount: impl FnOnce(u64) -> u64,
    ) -> Reservation {
        let amount = {
            let mut inner = self.inner.lock();

            let device_free = self.device_size().saturating_sub(inner.used_bytes());

            let amount = amount(match owner_object_id {
                Some(id) => std::cmp::min(inner.owner_id_bytes_left(id), device_free),
                None => device_free,
            });

            inner.add_reservation(owner_object_id, amount);

            amount
        };

        Reservation::new(self, owner_object_id, amount)
    }

    /// Returns the total number of allocated bytes.
    pub fn get_allocated_bytes(&self) -> u64 {
        self.inner.lock().allocated_bytes().0
    }

    /// Returns the size of bytes available to allocate.
    pub fn get_disk_bytes(&self) -> u64 {
        self.device_size()
    }

    /// The device's capacity, as it is now.
    fn device_size(&self) -> u64 {
        self.device_size.load(Ordering::Acquire)
    }

    /// The branch whose extents the allocator mints.
    fn branch(&self) -> u64 {
        self.branch.load(Ordering::Acquire)
    }

    /// fxdax: the allocator mints extents of `branch` from now on: the device's, as the filesystem
    /// thaws in a VM restored from a snapshot, whose store is a fork of its own, and nothing is
    /// open. Extents staged before, sealed, are not staged in again.
    pub fn set_branch(&self, branch: u64) {
        self.branch.store(branch, Ordering::Release);
    }

    /// fxdax: the device's capacity is `size` bytes from now on, as the extent store is resized
    /// while mounted: more, or fewer, but no fewer than the allocator holds, its extents alive,
    /// allocations not yet committed, and reservations, and its dead extents (`NoSpace`). The
    /// allocator's info records no size, so a mount after takes the device's.
    pub fn set_device_size(&self, size: u64) -> Result<(), Error> {
        let size = self.block_size.align_down(size);
        let inner = self.inner.lock();
        let held = inner.used_bytes().max(inner.unavailable_bytes());
        ensure!(
            size >= held,
            anyhow!(FxfsError::NoSpace)
                .context(format!("the allocator holds {held} bytes, more than {size}"))
        );
        self.device_size.store(size, Ordering::Release);
        Ok(())
    }

    /// Returns the total number of allocated bytes per owner_object_id.
    /// Note that this is quite an expensive operation as it copies the collection.
    /// This is intended for use in fsck() and friends, not general use code.
    pub fn get_owner_allocated_bytes(&self) -> BTreeMap<u64, u64> {
        self.inner.lock().owner_bytes.iter().map(|(k, v)| (*k, v.allocated_bytes.0)).collect()
    }

    /// Returns the number of bytes the extents alive take, whole, with those reserved and
    /// allocated but not yet committed.
    pub fn get_used_bytes(&self) -> Saturating<u64> {
        Saturating(self.inner.lock().used_bytes())
    }

    pub async fn flush(&self) -> Result<Version, Error> {
        let filesystem = self.filesystem.upgrade().unwrap();
        let object_manager = filesystem.object_manager();
        let earliest_version = self.tree.get_earliest_version();
        if !object_manager.needs_flush(self.object_id()) && earliest_version == LATEST_VERSION {
            // Early exit, but still return the earliest version used by a struct in the tree
            return Ok(earliest_version);
        }

        let fs = self.filesystem.upgrade().unwrap();
        let mut flusher = Flusher::new(self, &fs).await;
        let (new_layer_file, info) = flusher.start().await?;
        flusher.finish(new_layer_file, info).await
    }
}

impl ReservationOwner for Allocator {
    fn release_reservation(&self, owner_object_id: Option<u64>, amount: u64) {
        self.inner.lock().remove_reservation(owner_object_id, amount);
    }
}

impl Allocator {
    /// Deallocates `part`, which one reference held, as a transaction applied in `context` does,
    /// and returns the record of it: dead, kept until its extent dies so that the extent's size is
    /// known; or, if the rest of the extent is dead too, the extent's, which dies.
    fn deallocate_part(
        &self,
        inner: &mut Inner,
        part: ExtentRange,
        context: &ApplyContext<'_, '_>,
    ) -> Result<AllocatorItem, Error> {
        let log_file_offset = context.checkpoint.file_offset;
        Ok(match inner.remove_allocation(&part, log_file_offset)? {
            None => {
                let owner_object_id = inner.extents[&part.extent].owner_object_id;
                AllocatorItem {
                    key: AllocatorKey::new(part.extent, part.range),
                    value: AllocatorValue::Abs { count: 0, owner_object_id },
                }
            }
            Some((size, owner_object_id)) => {
                let free_after = if context.mode.is_live() {
                    FreeAfter::Flush(log_file_offset)
                } else {
                    FreeAfter::Now
                };
                inner.kill(free_after, part.extent, size, owner_object_id);
                // The device's room for the extent comes back once it is freed, which the
                // transaction can count on, as allocations wait for that.
                if let ApplyMode::Live(Transaction {
                    allocator_reservation: Some(reservation), ..
                }) = context.mode
                {
                    inner.add_reservation(reservation.owner_object_id(), size);
                    reservation.add(size);
                }
                AllocatorItem {
                    key: AllocatorKey::new(part.extent, 0..size),
                    value: AllocatorValue::None,
                }
            }
        })
    }
}

#[async_trait]
impl JournalingObject for Allocator {
    fn writes_mutations_as_they_are(&self) -> bool {
        true
    }

    fn apply_mutation(
        &self,
        mutation: Mutation,
        context: &ApplyContext<'_, '_>,
        _assoc_obj: AssocObj<'_>,
    ) -> Result<(), Error> {
        // Extents that die are freed once the device is flushed past this transaction (see
        // `FreeAfter`), or, for those whose deallocation replay finds, after replay.
        let log_file_offset = context.checkpoint.file_offset;
        let live = context.mode.is_live();
        match mutation {
            Mutation::Allocator(AllocatorMutation::MarkForDeletion(owner_object_id)) => {
                let mut inner = self.inner.lock();
                // fxdax: a store's extents are its current data owner's, and those of the frozen
                // owners it shares, which are deleted once no store shares them.
                let mut owners = vec![inner.data_owner(owner_object_id)];
                inner.info.data_owners.remove(&owner_object_id);
                let shared = inner.info.shared_owners.remove(&owner_object_id).unwrap_or_default();
                for frozen_owner in shared {
                    if let Some(sharers) = inner.info.frozen_owners.get_mut(&frozen_owner) {
                        *sharers = sharers.saturating_sub(1);
                        if *sharers == 0 {
                            inner.info.frozen_owners.remove(&frozen_owner);
                            owners.push(frozen_owner);
                        }
                    }
                }

                for owner in &owners {
                    inner.owner_bytes.remove(owner);
                    // The owner's allocations are ignored from now on, and filtered out of the
                    // tree by a compaction once its extents are freed.
                    inner.info.marked_for_deletion.insert(*owner);
                }
                inner.kill_owners(
                    &owners,
                    FreeAfter::RecordedFlush(if live { log_file_offset } else { 0 }),
                );

                inner.info.limit_bytes.remove(&owner_object_id);
            }
            Mutation::Allocator(AllocatorMutation::Snapshot {
                source_store_object_id,
                new_store_object_id,
                new_owner_object_id,
            }) => {
                // The new owner is an object id of the root store's: none may use it again.
                if context.mode.is_replay()
                    && let Some(filesystem) = self.filesystem.upgrade()
                {
                    filesystem.root_store().update_last_object_id(new_owner_object_id);
                }
                let mut inner = self.inner.lock();
                // The source's extents so far are frozen, and shared by both stores, with those the
                // source already shared.
                let frozen_owner = inner.data_owner(source_store_object_id);
                let mut shared = inner
                    .info
                    .shared_owners
                    .get(&source_store_object_id)
                    .cloned()
                    .unwrap_or_default();
                for owner in &shared {
                    *inner.info.frozen_owners.entry(*owner).or_default() += 1;
                }
                *inner.info.frozen_owners.entry(frozen_owner).or_default() += 2;
                shared.push(frozen_owner);
                inner.info.shared_owners.insert(source_store_object_id, shared.clone());
                inner.info.shared_owners.insert(new_store_object_id, shared);
                inner.info.data_owners.insert(source_store_object_id, new_owner_object_id);
                // What the source has reserved, or allocated in transactions not yet committed, is
                // counted under its new owner from now on.
                if let Some(frozen) = inner.owner_bytes.get_mut(&frozen_owner) {
                    let reserved = std::mem::take(&mut frozen.reserved_bytes);
                    let uncommitted = std::mem::take(&mut frozen.uncommitted_allocated_bytes);
                    let owner = inner.owner_bytes.entry(new_owner_object_id).or_default();
                    owner.reserved_bytes += reserved;
                    owner.uncommitted_allocated_bytes += uncommitted;
                }
            }
            Mutation::Allocator(AllocatorMutation::Allocate { device_range, owner_object_id }) => {
                // The allocation is the store's current data owner's.
                let mut inner = self.inner.lock();
                let data_owner = inner.data_owner(owner_object_id);
                // fxdax: see `Inner::unpin`.
                if context.mode.is_replay() {
                    inner.revive(device_range.extent);
                }
                inner.add_allocation(&device_range, data_owner)?;
                let len = device_range.len();
                let item = AllocatorItem {
                    key: AllocatorKey::new(device_range.extent, device_range.range),
                    value: AllocatorValue::Abs { count: 1, owner_object_id: data_owner },
                };
                let lower_bound = item.key.lower_bound_for_merge_into();
                self.tree.merge_into(item, &lower_bound);
                if let ApplyMode::Live(transaction) = context.mode {
                    inner.owner_bytes.entry(data_owner).or_default().uncommitted_allocated_bytes -=
                        len;
                    if let Some(reservation) = transaction.allocator_reservation {
                        reservation.commit(len);
                    }
                    inner.unpin(device_range.extent, &self.tree);
                }
            }
            Mutation::Allocator(AllocatorMutation::Reference { device_range, delta, .. }) => {
                let mut inner = self.inner.lock();
                let Some(extent) = inner.extents.get(&device_range.extent) else {
                    bail!(anyhow!(FxfsError::Inconsistent)
                        .context(format!("{device_range:?} referenced, but not allocated")));
                };
                for (part, count) in extent.references(device_range.range.clone()) {
                    let part = ExtentRange::new(device_range.extent, part);
                    let count = count.checked_add_signed(delta).ok_or_else(|| {
                        anyhow!(FxfsError::Inconsistent)
                            .context(format!("{part:?} referenced {delta} times more than {count}"))
                    })?;
                    let item = match count {
                        0 => self.deallocate_part(&mut inner, part, context)?,
                        count => inner.recount(part, count),
                    };
                    let lower_bound = item.key.lower_bound_for_merge_into();
                    self.tree.merge_into(item, &lower_bound);
                }
            }
            Mutation::Allocator(AllocatorMutation::Deallocate { device_range, .. }) => {
                let mut inner = self.inner.lock();
                // `deallocate` leaves out the extents frozen owners hold, so the extent is the
                // store's current data owner's. fxdax: the parts referenced more than once lose a
                // reference; the rest are deallocated (see "References").
                let parts = match inner.extents.get(&device_range.extent) {
                    Some(extent) => extent.references(device_range.range.clone()),
                    None => vec![(device_range.range.clone(), 1)],
                };
                for (part, count) in parts {
                    let part = ExtentRange::new(device_range.extent, part);
                    let item = match count {
                        1 => self.deallocate_part(&mut inner, part, context)?,
                        count => inner.recount(part, count - 1),
                    };
                    let lower_bound = item.key.lower_bound_for_merge_into();
                    self.tree.merge_into(item, &lower_bound);
                }
            }
            Mutation::Allocator(AllocatorMutation::SetLimit { owner_object_id, bytes }) => {
                // Journal replay is ordered and each of these calls is idempotent. So the last one
                // will be respected, it doesn't matter if the value is already set, or gets changed
                // multiple times during replay. When it gets opened it will be merged in with the
                // snapshot.
                self.inner.lock().info.limit_bytes.insert(owner_object_id, bytes);
            }
            Mutation::BeginFlush => {
                self.tree.seal();
                // Transfer our running count for allocated_bytes so that it gets written to the new
                // info file when flush completes.
                let mut inner = self.inner.lock();
                let allocated_bytes =
                    inner.owner_bytes.iter().map(|(k, v)| (*k, v.allocated_bytes.0)).collect();
                inner.info.allocated_bytes = allocated_bytes;
            }
            Mutation::EndFlush => {}
            _ => bail!("unexpected mutation: {:?}", mutation),
        }
        Ok(())
    }

    fn drop_mutation(&self, mutation: Mutation, transaction: &Transaction<'_>) {
        if let Mutation::Allocator(AllocatorMutation::Allocate { device_range, owner_object_id }) =
            mutation
        {
            let len = device_range.len();
            let mut inner = self.inner.lock();
            let data_owner = inner.data_owner(owner_object_id);
            inner.owner_bytes.entry(data_owner).or_default().uncommitted_allocated_bytes -= len;
            if let Some(reservation) = transaction.allocator_reservation {
                let res_owner = reservation.owner_object_id();
                inner.add_reservation(res_owner, len);
                reservation.release_reservation(res_owner, len);
            }
            // The extent's data may have been written: it is freed with the next flush. A shared
            // extent's other allocations keep it, until they go.
            if inner.shared.contains_key(&device_range.extent) {
                inner.unpin(device_range.extent, &self.tree);
            } else if device_range.extent.branch != 0 {
                inner.kill(FreeAfter::Now, device_range.extent, device_range.range.end, data_owner);
            }
        }
    }

    async fn flush(&self, _reason: crate::filesystem::FlushReason) -> Result<Version, Error> {
        self.flush().await
    }
}

// The merger is unable to merge extents that exist like the following:
//
//     |----- +1 -----|
//                    |----- -1 -----|
//                    |----- +2 -----|
//
// It cannot coalesce them because it has to emit the +1 record so that it can move on and merge the
// -1 and +2 records. To address this, we add another stage that applies after merging which
// coalesces records after they have been emitted.  This is a bit simpler than merging because the
// records cannot overlap, so it's just a question of merging adjacent records if they happen to
// have the same delta and object_id.

pub struct CoalescingIterator<I> {
    iter: I,
    item: Option<AllocatorItem>,
}

impl<I: LayerIterator<AllocatorKey, AllocatorValue>> CoalescingIterator<I> {
    pub async fn new(iter: I) -> Result<CoalescingIterator<I>, Error> {
        let mut iter = Self { iter, item: None };
        iter.advance().await?;
        Ok(iter)
    }
}

impl<I: LayerIterator<AllocatorKey, AllocatorValue>> LayerIterator<AllocatorKey, AllocatorValue>
    for CoalescingIterator<I>
{
    async fn advance(&mut self) -> Result<(), Error> {
        self.item = self.iter.get().map(|x| x.cloned());
        if self.item.is_none() {
            return Ok(());
        }
        let left = self.item.as_mut().unwrap();
        loop {
            self.iter.advance().await?;
            match self.iter.get() {
                None => return Ok(()),
                Some(right) => {
                    // The two records cannot overlap.
                    ensure!(left.key.is_before(right.key), FxfsError::Inconsistent);
                    // We can only coalesce records if they are touching, in the same extent, and
                    // have the same value.
                    if left.key.extent != right.key.extent
                        || left.key.range.end < right.key.range.start
                        || left.value != *right.value
                    {
                        return Ok(());
                    }
                    left.key.range.end = right.key.range.end;
                }
            }
        }
    }

    fn advance_dyn<'a>(&'a mut self) -> Result<Option<BoxFuture<'a, Result<(), Error>>>, Error> {
        Ok(Some(Box::pin(self.advance())))
    }

    fn get(&self) -> Option<ItemRef<'_, AllocatorKey, AllocatorValue>> {
        self.item.as_ref().map(|x| x.as_item_ref())
    }
}

struct Flusher<'a> {
    allocator: &'a Allocator,
    fs: &'a Arc<FxFilesystem>,
    _guard: WriteGuard<'a>,
}

impl<'a> Flusher<'a> {
    async fn new(allocator: &'a Allocator, fs: &'a Arc<FxFilesystem>) -> Self {
        let keys = lock_keys![LockKey::flush(allocator.object_id())];
        Self { allocator, fs, _guard: fs.lock_manager().write_lock(keys).await }
    }

    fn txn_options(allocator_reservation: &Reservation) -> Options<'_> {
        Options {
            skip_journal_checks: true,
            borrow_metadata_space: true,
            allocator_reservation: Some(allocator_reservation),
            ..Default::default()
        }
    }

    async fn start(&mut self) -> Result<(DataObjectHandle<ObjectStore>, AllocatorInfo), Error> {
        let object_manager = self.fs.object_manager();
        let root_store = self.fs.root_store();
        let mut transaction = root_store
            .new_transaction(lock_keys![], Self::txn_options(object_manager.metadata_reservation()))
            .await?;
        let layer_object_handle = ObjectStore::create_object(
            &root_store,
            &mut transaction,
            HandleOptions { skip_journal_checks: true, ..Default::default() },
            None,
        )
        .await?;
        root_store.add_to_graveyard(&mut transaction, layer_object_handle.object_id());
        // It's important that this transaction does not include any allocations because we use
        // BeginFlush as a snapshot point for mutations to the tree: other allocator mutations
        // within this transaction might get applied before seal (which would be OK), but they could
        // equally get applied afterwards (since Transaction makes no guarantees about the order in
        // which mutations are applied whilst committing), in which case they'd get lost on replay
        // because the journal will only send mutations that follow this transaction.
        transaction.add(self.allocator.object_id(), Mutation::BeginFlush);
        let info = transaction
            .commit_with_callback(|_| {
                // We must capture `info` as it is when we start flushing. Subsequent transactions
                // can end up modifying `info` and we shouldn't capture those here.
                // fxdax: with the extents dying, which the compaction leaves out.
                let inner = self.allocator.inner.lock();
                AllocatorInfo { dying: inner.dying(), ..inner.info.clone() }
            })
            .await?;
        Ok((layer_object_handle, info))
    }

    async fn finish(
        self,
        layer_object_handle: DataObjectHandle<ObjectStore>,
        mut info: AllocatorInfo,
    ) -> Result<Version, Error> {
        let object_manager = self.fs.object_manager();
        let txn_options = Self::txn_options(object_manager.metadata_reservation());

        // fxdax: the owners marked for deletion whose extents have all been freed, whose
        // allocations the compaction filters out. The others' stay until their extents are freed,
        // which a mount does if the filesystem doesn't first.
        let freed_owners: HashSet<u64> = {
            let inner = self.allocator.inner.lock();
            info.marked_for_deletion
                .iter()
                .copied()
                .filter(|owner| !inner.dead.iter().any(|dead| dead.owner_object_id == *owner))
                .collect()
        };

        let layer_set = self.allocator.tree.immutable_layer_set();
        let total_len = layer_set.sum_len();
        {
            let start_time = std::time::Instant::now();
            let merged_layer_count = layer_set.layers.len();
            let mut merger = layer_set.merger();
            let iter = filter_marked_for_deletion(
                filter_tombstones(merger.query(Query::FullScan).await?).await?,
                freed_owners.clone(),
            )
            .await?;
            let iter = CoalescingIterator::new(iter).await?;
            let bytes_written = compact_with_iterator(
                iter,
                total_len,
                DirectWriter::new(&layer_object_handle, txn_options).await,
                layer_object_handle.block_size(),
                Some(self.fs.journal().get_compaction_yielder()),
            )
            .await?;

            self.allocator.tree.report_compaction_metrics(
                bytes_written,
                start_time.elapsed(),
                merged_layer_count,
            );
        }

        let root_store = self.fs.root_store();

        // Both of these forward-declared variables need to outlive the transaction.
        let object_handle;
        let reservation_update;
        let mut transaction = root_store
            .new_transaction(
                lock_keys![LockKey::object(
                    root_store.store_object_id(),
                    self.allocator.object_id()
                )],
                txn_options,
            )
            .await?;
        let mut serialized_info = Vec::new();

        debug!(oid = layer_object_handle.object_id(); "new allocator layer file");
        object_handle = ObjectStore::open_object(
            &root_store,
            self.allocator.object_id(),
            HandleOptions::default(),
            None,
        )
        .await?;

        // Move all the existing layers to the graveyard.
        for object_id in &info.layers {
            root_store.add_to_graveyard(&mut transaction, *object_id);
        }

        // Write out updated info.

        // After successfully flushing, all the stores that were marked for deletion at the time of
        // the BeginFlush transaction, and whose extents have been freed, no longer need to be
        // marked for deletion.  There can be stores that have been deleted since the BeginFlush
        // transaction, but they will be covered by a MarkForDeletion mutation.
        info.marked_for_deletion.retain(|owner| !freed_owners.contains(owner));

        info.layers = vec![layer_object_handle.object_id()];

        info.serialize_with_version(&mut serialized_info)?;

        let mut buf = object_handle.allocate_buffer(serialized_info.len()).await;
        buf.copy_from_slice(&serialized_info);
        object_handle.txn_write(&mut transaction, 0u64, buf.as_ref()).await?;

        reservation_update = ReservationUpdate::new(tree::reservation_amount_from_layer_size(
            layer_object_handle.get_size(),
        ));

        // It's important that EndFlush is in the same transaction that we write AllocatorInfo,
        // because we use EndFlush to make the required adjustments to allocated_bytes.
        transaction.add_with_object(
            self.allocator.object_id(),
            Mutation::EndFlush,
            AssocObj::Borrowed(&reservation_update),
        );
        root_store.remove_from_graveyard(&mut transaction, layer_object_handle.object_id());

        let layers = vec![layer_from_handle(layer_object_handle, None).await?];
        transaction
            .commit_with_callback(|_| {
                self.allocator.tree.set_layers(layers);

                // At this point we've committed the new layers to disk so we can start using them.
                // This means we can also switch to the new AllocatorInfo which clears
                // marked_for_deletion.
                let mut inner = self.allocator.inner.lock();
                inner.info.layers = info.layers;
                for owner_id in freed_owners {
                    inner.info.marked_for_deletion.remove(&owner_id);
                }
            })
            .await?;

        // Now close the layers and purge them.
        for layer in layer_set.layers {
            let object_id = layer.handle().map(|h| h.object_id());
            layer.close_layer().await;
            if let Some(object_id) = object_id {
                root_store.tombstone_object(object_id, txn_options, None).await?;
            }
        }

        let mut counters = self.allocator.counters.lock();
        counters.num_flushes += 1;
        counters.last_flush_time = Some(std::time::SystemTime::now());
        // Return the earliest version used by a struct in the tree
        Ok(self.allocator.tree.get_earliest_version())
    }
}

#[cfg(test)]
mod tests {
    use super::TEST_EXTENT;
    use crate::filesystem::{FxFilesystem, FxFilesystemBuilder, OpenFxFilesystem, SyncOptions};
    use crate::fsck::fsck;
    use crate::lsm_tree::skip_list_layer::SkipListLayer;
    use crate::lsm_tree::types::{FuzzyHash as _, Item, ItemRef, LayerIterator, LayerKey as _};
    use crate::lsm_tree::{LSMTree, Query};
    use crate::object_handle::ObjectHandle;
    use crate::object_store::allocator::merge::merge;
    use crate::object_store::allocator::{
        Allocator, AllocatorKey, AllocatorValue, CoalescingIterator, EXTENT_HASH_BUCKET_SIZE,
        STAGING_SIZE,
    };
    use crate::object_store::extent::MIN_BLOCK_SIZE;
    use crate::object_store::transaction::{
        Options, TRANSACTION_METADATA_MAX_AMOUNT, Transaction, lock_keys,
    };
    use crate::object_store::volume::root_volume;
    use crate::object_store::{Directory, LockKey, NewChildStoreOptions, ObjectStore};
    use crate::serialized_types::{LATEST_VERSION, Versioned};
    use crate::testing;
    use bincode::Options as _;
    use fuchsia_async as fasync;
    use std::cmp::{max, min};
    use std::ops::Bound;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use storage_device::fake_device::{DEFAULT_BRANCH, FakeDevice, Op};
    use storage_device::{Device, DeviceHolder, ExtentId, ExtentRange};

    /// fxdax: the allocator information of images before snapshots is read as it was, with no
    /// snapshots, and minting from the start.
    #[test]
    fn allocator_info_migrates_from_version_32() {
        use crate::object_store::allocator::{AllocatorInfo, AllocatorInfoV32};
        use crate::serialized_types::{Version, VersionedLatest};
        let old = AllocatorInfoV32 {
            layers: vec![7, 8],
            allocated_bytes: [(10, 4096)].into(),
            marked_for_deletion: [11].into(),
            limit_bytes: [(10, 1 << 20)].into(),
        };
        let mut serialized = Vec::new();
        old.serialize_into(&mut serialized).expect("serialize failed");
        let info = AllocatorInfo::deserialize_from_version(
            &mut std::io::Cursor::new(serialized),
            Version { major: 58, minor: 0 },
        )
        .expect("deserialize failed");
        assert_eq!(info.layers, vec![7, 8]);
        assert_eq!(info.allocated_bytes, [(10, 4096)].into());
        assert_eq!(info.marked_for_deletion, [11].into());
        assert_eq!(info.limit_bytes, [(10, 1 << 20)].into());
        assert!(info.data_owners.is_empty());
        assert!(info.frozen_owners.is_empty());
        assert!(info.shared_owners.is_empty());
        assert_eq!(info.next_sequence, 0);
    }

    #[test]
    fn test_allocator_key_is_range_based() {
        // Make sure we disallow using allocator keys with point queries.
        assert!(AllocatorKey::new(TEST_EXTENT, 0..100).is_range_key());
    }

    #[test]
    fn test_allocator_key_fuzzy_hash_len() {
        let key = AllocatorKey::new(TEST_EXTENT, 0..512);
        let mut iter = key.fuzzy_hash();
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);

        let key = AllocatorKey::new(TEST_EXTENT, 0..3 * EXTENT_HASH_BUCKET_SIZE);
        let mut iter = key.fuzzy_hash();
        assert_eq!(iter.len(), 3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 2);
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert!(iter.next().is_some());
        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn test_allocator_key_search_key() {
        let key = AllocatorKey::new(TEST_EXTENT, MIN_BLOCK_SIZE.get()..3 * MIN_BLOCK_SIZE);
        assert!(!key.is_search_key());
        let search_key = key.search_key().unwrap();
        assert!(search_key.is_search_key());
        assert_eq!(
            search_key,
            AllocatorKey::new(TEST_EXTENT, MIN_BLOCK_SIZE.get()..2 * MIN_BLOCK_SIZE)
        );
    }

    /// fxdax: a key is its extent's ID and the range within it, which bincode serializes as it does
    /// the pair.
    #[test]
    fn test_allocator_key_serialization_compatibility() {
        let range = 1024..2048;
        let key = AllocatorKey::new(TEST_EXTENT, range.clone());

        // 1. Serialize the extent and the range directly using bincode.
        let options = bincode::DefaultOptions::new().allow_trailing_bytes();
        let expected_bytes = options.serialize(&(TEST_EXTENT, range)).unwrap();

        // 2. Serialize AllocatorKey via our derived Versioned implementation
        let mut key_bytes = Vec::new();
        key.serialize_into(&mut key_bytes).unwrap();

        // Verify they are byte-compatible
        assert_eq!(key_bytes, expected_bytes);

        // 3. Deserialize the pair as AllocatorKey via our derived Versioned implementation
        let deserialized_key =
            AllocatorKey::deserialize_from(&mut &expected_bytes[..], LATEST_VERSION).unwrap();
        assert_eq!(deserialized_key, key);
    }

    #[fuchsia::test]
    async fn test_coalescing_iterator() {
        let skip_list = SkipListLayer::new(100);
        let items = [
            Item::new(
                AllocatorKey::new(TEST_EXTENT, 0..100 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ),
            Item::new(
                AllocatorKey::new(TEST_EXTENT, 100 * 512..200 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ),
        ];
        skip_list.insert(items[1].clone()).expect("insert error");
        skip_list.insert(items[0].clone()).expect("insert error");
        let mut iter =
            CoalescingIterator::new(skip_list.seek(Bound::Unbounded)).await.expect("new failed");
        let ItemRef { key, value, .. } = iter.get().expect("get failed");
        assert_eq!(
            (key, value),
            (
                &AllocatorKey::new(TEST_EXTENT, 0..200 * 512),
                &AllocatorValue::Abs { count: 1, owner_object_id: 99 }
            )
        );
        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_and_coalesce_across_three_layers() {
        let lsm_tree = LSMTree::new(merge, None);
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(TEST_EXTENT, 100 * 512..200 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ))
            .expect("insert error");
        lsm_tree.seal();
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(TEST_EXTENT, 0..100 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ))
            .expect("insert error");

        let layer_set = lsm_tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter =
            CoalescingIterator::new(merger.query(Query::FullScan).await.expect("seek failed"))
                .await
                .expect("new failed");
        let ItemRef { key, value, .. } = iter.get().expect("get failed");
        assert_eq!(
            (key, value),
            (
                &AllocatorKey::new(TEST_EXTENT, 0..200 * 512),
                &AllocatorValue::Abs { count: 1, owner_object_id: 99 }
            )
        );
        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    #[fuchsia::test]
    async fn test_merge_and_coalesce_wont_merge_across_object_id() {
        let lsm_tree = LSMTree::new(merge, None);
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(TEST_EXTENT, 100 * 512..200 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ))
            .expect("insert error");
        lsm_tree.seal();
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(TEST_EXTENT, 0..100 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 98 },
            ))
            .expect("insert error");

        let layer_set = lsm_tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter =
            CoalescingIterator::new(merger.query(Query::FullScan).await.expect("seek failed"))
                .await
                .expect("new failed");
        let ItemRef { key, value, .. } = iter.get().expect("get failed");
        assert_eq!(
            (key, value),
            (
                &AllocatorKey::new(TEST_EXTENT, 0..100 * 512),
                &AllocatorValue::Abs { count: 1, owner_object_id: 98 },
            )
        );
        iter.advance().await.expect("advance failed");
        let ItemRef { key, value, .. } = iter.get().expect("get failed");
        assert_eq!(
            (key, value),
            (
                &AllocatorKey::new(TEST_EXTENT, 100 * 512..200 * 512),
                &AllocatorValue::Abs { count: 1, owner_object_id: 99 }
            )
        );
        iter.advance().await.expect("advance failed");
        assert!(iter.get().is_none());
    }

    /// fxdax: keys sort by extent, and within an extent, by range; records of different extents
    /// never coalesce.
    #[fuchsia::test]
    async fn test_merge_and_coalesce_wont_merge_across_extents() {
        let lsm_tree = LSMTree::new(merge, None);
        let (a, b) = (ExtentId::new(1, 2), ExtentId::new(1, 3));
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(b, 0..100 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ))
            .expect("insert error");
        lsm_tree.seal();
        lsm_tree
            .insert(Item::new(
                AllocatorKey::new(a, 0..100 * 512),
                AllocatorValue::Abs { count: 1, owner_object_id: 99 },
            ))
            .expect("insert error");

        let layer_set = lsm_tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter =
            CoalescingIterator::new(merger.query(Query::FullScan).await.expect("seek failed"))
                .await
                .expect("new failed");
        for extent in [a, b] {
            let ItemRef { key, value, .. } = iter.get().expect("get failed");
            assert_eq!(
                (key, value),
                (
                    &AllocatorKey::new(extent, 0..100 * 512),
                    &AllocatorValue::Abs { count: 1, owner_object_id: 99 }
                )
            );
            iter.advance().await.expect("advance failed");
        }
        assert!(iter.get().is_none());
    }

    fn overlap(a: &ExtentRange, b: &ExtentRange) -> u64 {
        if a.extent == b.extent && a.range.end > b.range.start && a.range.start < b.range.end {
            min(a.range.end, b.range.end) - max(a.range.start, b.range.start)
        } else {
            0
        }
    }

    /// The ranges allocated, in order: those of extents still alive that are deallocated aren't.
    async fn collect_allocations(allocator: &Allocator) -> Vec<ExtentRange> {
        let layer_set = allocator.tree.layer_set();
        let mut merger = layer_set.merger();
        let mut iter = allocator
            .filter(merger.query(Query::FullScan).await.expect("seek failed"))
            .await
            .expect("build iterator");
        let mut allocations: Vec<ExtentRange> = Vec::new();
        while let Some(ItemRef { key, value, .. }) = iter.get() {
            if let AllocatorValue::Abs { count: 0, .. } = value {
                iter.advance().await.expect("advance failed");
                continue;
            }
            if let Some(r) = allocations.last() {
                assert!(r.extent < key.extent || key.range.start >= r.range.end);
            }
            allocations.push(key.extent_range());
            iter.advance().await.expect("advance failed");
        }
        allocations
    }

    async fn check_allocations(allocator: &Allocator, expected_allocations: &[ExtentRange]) {
        let mut found = 0;
        for range in collect_allocations(allocator).await {
            let mut l = range.len();
            found += l;
            // Make sure that the entire range we have found completely overlaps with all the
            // allocations we expect to find.
            for expected in expected_allocations {
                l -= overlap(expected, &range);
                if l == 0 {
                    break;
                }
            }
            assert_eq!(l, 0, "range {:?} not covered by expectations", range);
        }
        // Make sure the total we found adds up to what we expect.
        assert_eq!(found, expected_allocations.iter().map(|r| r.len()).sum::<u64>());
    }

    /// Whether the device holds `extent`.
    async fn device_holds(device: &Arc<dyn Device>, extent: ExtentId) -> bool {
        let mut buffer = device.allocate_buffer(device.block_size() as usize).await;
        device.read(extent, 0, buffer.as_mut()).await.is_ok()
    }

    /// Commits a transaction allocating a block for a store, and returns the allocation.
    async fn commit_allocation(fs: &OpenFxFilesystem) -> ExtentRange {
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = fs
            .allocator()
            .allocate(&mut transaction, 99, fs.block_size().get())
            .await
            .expect("allocate failed");
        transaction.commit().await.expect("commit failed");
        range
    }

    async fn test_fs() -> (OpenFxFilesystem, Arc<Allocator>) {
        let device = DeviceHolder::new(FakeDevice::new(4096, 4096));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        let allocator = fs.allocator();
        (fs, allocator)
    }

    fn extent(sequence: u64) -> ExtentId {
        ExtentId::new(DEFAULT_BRANCH, sequence)
    }

    /// fxdax: a filesystem reopened on a device whose floor is far past what it holds, as when
    /// another writer of its branch takes it over (backlog doc-7, Fencing), mints from the floor,
    /// and looks for extents a crash left below what the device holds only: not across the gap to
    /// the floor, which took a guest's allocator ever.
    #[fuchsia::test]
    async fn test_mints_past_the_sequence_floor() {
        const STORE_OBJECT_ID: u64 = 99;
        let floor = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let device =
            DeviceHolder::new(FakeDevice::new(4096, 4096).with_sequence_floor(floor.clone()));
        let fs = FxFilesystem::new_empty(device).await.expect("new_empty failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let held = device.next_sequence().await.expect("next_sequence failed");
        floor.store(1 << 40, Ordering::Relaxed);

        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = fs
            .allocator()
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");
        assert_eq!(range, ExtentRange::new(extent(1 << 40), 0..4096));
        assert!(held < 1 << 10, "the device held {held} sequence numbers");
        transaction.commit().await.expect("commit failed");
        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_allocations() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let mut device_ranges = collect_allocations(&allocator).await;

        // Expected extents, in the order they were minted: the journal's blocks are extents of
        // their own.
        let block = |sequence| ExtentRange::new(extent(sequence), 0..4096);
        let expected: Vec<_> = (0..32) // The initial journal (128k)
            .chain([32, 33]) // The root store's info, and superblock A
            .chain(34..66) // The journal's next 128k
            .chain([66]) // Superblock B
            .map(block)
            .collect();
        assert_eq!(device_ranges, expected);
        let next_sequence = allocator.inner.lock().next_sequence;
        for i in 0..2 {
            let range = allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed");
            // Each allocation is an extent of its own, which it starts.
            assert_eq!(range, ExtentRange::new(extent(next_sequence + i), 0..4096));
            device_ranges.push(range);
        }
        transaction.commit().await.expect("commit failed");
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        device_ranges.push(
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed"),
        );
        assert_eq!(device_ranges.last().unwrap().extent, extent(next_sequence + 2));
        transaction.commit().await.expect("commit failed");

        check_allocations(&allocator, &device_ranges).await;
    }

    #[fuchsia::test]
    async fn test_allocate_more_than_max_size() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let mut device_ranges = collect_allocations(&allocator).await;
        device_ranges.push(
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.device().size())
                .await
                .expect("allocate failed"),
        );
        assert_eq!(device_ranges.last().unwrap().len(), allocator.max_extent_size_bytes);
        transaction.commit().await.expect("commit failed");

        check_allocations(&allocator, &device_ranges).await;
    }

    /// An extent dies once it is deallocated, and is freed once the device has been flushed.
    #[fuchsia::test]
    async fn test_deallocations() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let initial_allocations = collect_allocations(&allocator).await;

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let device_range1 = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");
        assert_eq!(device_range1.len(), fs.block_size());
        transaction.commit().await.expect("commit failed");
        let device = fs.device();
        let mut buffer = device.allocate_buffer(fs.block_size().get() as usize).await;
        device.write(device_range1.extent, 0, buffer.as_ref()).await.expect("write failed");

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, device_range1.clone())
            .await
            .expect("deallocate failed");
        transaction.commit().await.expect("commit failed");

        check_allocations(&allocator, &initial_allocations).await;
        assert_eq!(allocator.dead_extents(), vec![device_range1.extent]);
        assert!(device_holds(&device, device_range1.extent).await);

        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        assert_eq!(allocator.dead_extents(), vec![]);
        assert!(!device_holds(&device, device_range1.extent).await);
        assert!(device.read(device_range1.extent, 0, buffer.as_mut()).await.is_err());
    }

    /// fxdax: an extent part of which is deallocated is alive until the rest is, and keeps its
    /// size meanwhile, across remounts.
    #[fuchsia::test]
    async fn test_partial_deallocations() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let bs = fs.block_size();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, 4 * bs)
            .await
            .expect("allocate failed");
        transaction.commit().await.expect("commit failed");
        let extent_bytes = allocator.inner.lock().extent_bytes;

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let tail = ExtentRange::new(range.extent, 2 * bs..4 * bs);
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, tail)
            .await
            .expect("deallocate failed");
        transaction.commit().await.expect("commit failed");
        // The extent takes its whole size until it dies.
        assert_eq!(allocator.inner.lock().extent_bytes, extent_bytes);
        assert!(allocator.dead_extents().is_empty());
        assert_eq!(allocator.extents()[&range.extent], (STORE_OBJECT_ID, 2 * bs));

        allocator.flush().await.expect("flush failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        let allocator = fs.allocator();
        assert_eq!(allocator.extents()[&range.extent], (STORE_OBJECT_ID, 2 * bs));

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let head = ExtentRange::new(range.extent, 0..2 * bs);
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, head)
            .await
            .expect("deallocate failed");
        transaction.commit().await.expect("commit failed");
        assert!(!allocator.extents().contains_key(&range.extent));
        assert_eq!(allocator.dead_extents(), vec![range.extent]);
        fs.close().await.expect("close failed");
    }

    /// fxdax: a transaction that is dropped kills the extents it allocated, which are never minted
    /// again.
    #[fuchsia::test]
    async fn test_mark_allocated() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let mut device_ranges = collect_allocations(&allocator).await;
        let range = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new failed");
            // First, allocate 2 blocks.
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, 2 * fs.block_size())
                .await
                .expect("allocate failed")
            // Let the transaction drop.
        };
        assert_eq!(allocator.dead_extents(), vec![range.extent]);

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");

        // Allocating again mints another extent.
        device_ranges.push(
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed"),
        );
        assert!(device_ranges.last().unwrap().extent > range.extent);

        // Mark an extent no allocator mints as allocated.
        let marked = ExtentRange::new(TEST_EXTENT, 0..fs.block_size().get());
        allocator
            .mark_allocated(&mut transaction, STORE_OBJECT_ID, marked.clone())
            .expect("mark_allocated failed");
        device_ranges.push(marked.clone());
        // An extent allocators mint can't be.
        let minted = ExtentRange::new(range.extent, 0..fs.block_size().get());
        allocator
            .mark_allocated(&mut transaction, STORE_OBJECT_ID, minted)
            .expect_err("marked a minted extent allocated");
        transaction.commit().await.expect("commit failed");

        // Nor can an extent that is allocated.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .mark_allocated(&mut transaction, STORE_OBJECT_ID, marked)
            .expect_err("marked an allocated extent allocated");

        check_allocations(&allocator, &device_ranges).await;
    }

    #[fuchsia::test]
    async fn test_mark_for_deletion() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;

        // Allocate some stuff.
        let initial_allocated_bytes = allocator.get_allocated_bytes();
        let initial_allocations = collect_allocations(&allocator).await;
        let mut device_ranges = initial_allocations.clone();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        // Note we have a cap on individual allocation length so we allocate over multiple mutation.
        for _ in 0..15 {
            device_ranges.push(
                allocator
                    .allocate(&mut transaction, STORE_OBJECT_ID, 100 * fs.block_size())
                    .await
                    .expect("allocate failed"),
            );
            device_ranges.push(
                allocator
                    .allocate(&mut transaction, STORE_OBJECT_ID, 100 * fs.block_size())
                    .await
                    .expect("allocate2 failed"),
            );
        }
        transaction.commit().await.expect("commit failed");
        check_allocations(&allocator, &device_ranges).await;

        assert_eq!(
            allocator.get_allocated_bytes(),
            initial_allocated_bytes + fs.block_size() * 3000
        );

        // Mark for deletion.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator.mark_for_deletion(&mut transaction, STORE_OBJECT_ID);
        transaction.commit().await.expect("commit failed");

        // fxdax: expect that allocated bytes is updated immediately, and the owner's allocations
        // ignored, but its extents are only dead: they are freed once the device is flushed.
        assert_eq!(allocator.get_allocated_bytes(), initial_allocated_bytes);
        check_allocations(&allocator, &initial_allocations).await;
        assert_eq!(allocator.dead_extents().len(), 30);

        // Allocate more space than we have until we free the mark_for_deletion space.
        // This should force a flush on allocate(). (1500 * 3 > test_fs size of 4096 blocks).
        device_ranges.clear();

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let target_bytes = 1500 * fs.block_size();
        while device_ranges.iter().map(|x| x.len()).sum::<u64>() != target_bytes {
            let len = std::cmp::min(
                target_bytes - device_ranges.iter().map(|x| x.len()).sum::<u64>(),
                100 * fs.block_size(),
            );
            device_ranges.push(
                allocator.allocate(&mut transaction, 100, len).await.expect("allocate failed"),
            );
        }
        transaction.commit().await.expect("commit failed");
        assert!(allocator.dead_extents().is_empty());

        // Have the deleted ranges cleaned up.
        allocator.flush().await.expect("flush failed");

        // The flush above seems to trigger an allocation for the allocator itself.
        // We will just check that we have the right size for the owner we care about.

        assert!(!allocator.get_owner_allocated_bytes().contains_key(&STORE_OBJECT_ID));
        assert_eq!(*allocator.get_owner_allocated_bytes().get(&100).unwrap(), target_bytes,);
        assert!(allocator.info().marked_for_deletion.is_empty());
    }

    async fn create_file(store: &Arc<ObjectStore>, size: usize) {
        let root_directory =
            Directory::open(store, store.root_directory_object_id()).await.expect("open failed");

        let mut transaction = store
            .filesystem()
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(
                    store.store_object_id(),
                    store.root_directory_object_id()
                )],
                Options::default(),
            )
            .await
            .expect("new_transaction failed");
        let file = root_directory
            .create_child_file(&mut transaction, &format!("foo {}", size))
            .await
            .expect("create_child_file failed");
        transaction.commit().await.expect("commit failed");

        let buffer = file.allocate_buffer(size).await;

        // Append some data to it.
        let mut transaction = file
            .new_transaction_with_options(Options {
                borrow_metadata_space: true,
                ..Default::default()
            })
            .await
            .expect("new_transaction_with_options failed");
        file.txn_write(&mut transaction, 0, buffer.as_ref()).await.expect("txn_write failed");
        transaction.commit().await.expect("commit failed");
    }

    #[fuchsia::test]
    async fn test_replay_with_deleted_store_and_compaction() {
        let (fs, _) = test_fs().await;

        const FILE_SIZE: usize = 10_000_000;

        let mut store_id = {
            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .new_volume("vol", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");

            create_file(&store, FILE_SIZE).await;
            store.store_object_id()
        };

        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);

        let mut fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");

        // Compact so that when we replay the transaction to delete the store won't find any
        // mutations.
        fs.journal().force_compact().await.expect("compact failed");

        for _ in 0..2 {
            {
                let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");

                let transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![
                            LockKey::object(
                                root_vol.volume_directory().store().store_object_id(),
                                root_vol.volume_directory().object_id(),
                            ),
                            LockKey::flush(store_id)
                        ],
                        Options { borrow_metadata_space: true, ..Default::default() },
                    )
                    .await
                    .expect("new_transaction failed");
                root_vol
                    .delete_volume("vol", transaction, || {})
                    .await
                    .expect("delete_volume failed");

                let store = root_vol
                    .new_volume("vol", NewChildStoreOptions::default())
                    .await
                    .expect("new_volume failed");
                create_file(&store, FILE_SIZE).await;
                store_id = store.store_object_id();
            }

            fs.close().await.expect("close failed");
            let device = fs.take_device().await;
            device.reopen(false);

            fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        }

        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    #[fuchsia::test(threads = 4)]
    async fn test_compaction_delete_race() {
        let (fs, _allocator) = test_fs().await;

        {
            const FILE_SIZE: usize = 10_000_000;

            let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
            let store = root_vol
                .new_volume("vol", NewChildStoreOptions::default())
                .await
                .expect("new_volume failed");

            create_file(&store, FILE_SIZE).await;

            // Race compaction with deleting a store.
            let fs_clone = fs.clone();

            // Even though the executor has 4 threads, it's hard to get it to run with
            // multiple threads.
            let executor_tasks = testing::force_executor_threads_to_run(4).await;

            let task = fasync::Task::spawn(async move {
                fs_clone.journal().force_compact().await.expect("compact failed");
            });

            // We don't need the executor tasks any more.
            drop(executor_tasks);

            // This range is chosen such that it caused this test to fail after quite a low number
            // of iterations for the bug that this test was introduced for.
            let sleep = rand::random_range(3000..6000);
            std::thread::sleep(std::time::Duration::from_micros(sleep));
            log::info!("sleep {sleep}us");

            let transaction = fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(
                            root_vol.volume_directory().store().store_object_id(),
                            root_vol.volume_directory().object_id(),
                        ),
                        LockKey::flush(store.store_object_id())
                    ],
                    Options { borrow_metadata_space: true, ..Default::default() },
                )
                .await
                .expect("new_transaction failed");
            root_vol.delete_volume("vol", transaction, || {}).await.expect("delete_volume failed");

            task.await;
        }

        fs.journal().force_compact().await.expect("compact failed");
        fs.close().await.expect("close failed");

        let device = fs.take_device().await;
        device.reopen(false);

        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    #[fuchsia::test]
    async fn test_delete_multiple_volumes() {
        let (mut fs, _) = test_fs().await;

        for _ in 0..50 {
            {
                let root_vol = root_volume(fs.clone()).await.expect("root_volume failed");
                let store = root_vol
                    .new_volume("vol", NewChildStoreOptions::default())
                    .await
                    .expect("new_volume failed");

                create_file(&store, 1_000_000).await;

                let transaction = fs
                    .root_store()
                    .new_transaction(
                        lock_keys![
                            LockKey::object(
                                root_vol.volume_directory().store().store_object_id(),
                                root_vol.volume_directory().object_id(),
                            ),
                            LockKey::flush(store.store_object_id())
                        ],
                        Options { borrow_metadata_space: true, ..Default::default() },
                    )
                    .await
                    .expect("new_transaction failed");
                root_vol
                    .delete_volume("vol", transaction, || {})
                    .await
                    .expect("delete_volume failed");

                fs.allocator().flush().await.expect("flush failed");
            }

            fs.close().await.expect("close failed");
            let device = fs.take_device().await;
            device.reopen(false);

            fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        }

        fsck(fs.clone()).await.expect("fsck failed");
        fs.close().await.expect("close failed");
    }

    /// fxdax: the device's capacity changes while mounted: it grows, and shrinks no lower than
    /// what the allocator holds, which allocations then fit within.
    #[fuchsia::test]
    async fn test_device_size_changes() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let size = allocator.get_disk_bytes();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, 100 * fs.block_size())
            .await
            .expect("allocate failed");
        transaction.commit().await.expect("commit failed");
        // What it holds, reservations included, in whole blocks.
        let held = allocator.get_used_bytes().0.next_multiple_of(fs.block_size().get());

        allocator.set_device_size(2 * size).expect("grow");
        assert_eq!(allocator.get_disk_bytes(), 2 * size);
        let refused = allocator.set_device_size(held - fs.block_size()).expect_err("shrink");
        assert!(crate::errors::FxfsError::NoSpace.matches(&refused), "{refused:?}");
        assert_eq!(allocator.get_disk_bytes(), 2 * size);
        allocator.set_device_size(held).expect("shrink to what it holds");
        assert_eq!(allocator.get_disk_bytes(), held);
        // Nothing more fits: not even a transaction's reservation.
        let full = fs.root_store().new_transaction(lock_keys![], Options::default()).await;
        assert!(crate::errors::FxfsError::NoSpace.matches(&full.err().expect("no room")));
        allocator.set_device_size(size).expect("grow again");
        fs.root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("room again");
    }

    #[fuchsia::test]
    async fn test_allocate_free_reallocate() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;

        // Allocate some stuff.
        let mut device_ranges = Vec::new();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        for _ in 0..30 {
            device_ranges.push(
                allocator
                    .allocate(&mut transaction, STORE_OBJECT_ID, 100 * fs.block_size())
                    .await
                    .expect("allocate failed"),
            );
        }
        transaction.commit().await.expect("commit failed");

        assert_eq!(
            fs.block_size() * 3000,
            *allocator.get_owner_allocated_bytes().entry(STORE_OBJECT_ID).or_default()
        );

        // Delete it all.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        for range in std::mem::replace(&mut device_ranges, Vec::new()) {
            allocator.deallocate(&mut transaction, STORE_OBJECT_ID, range).await.expect("dealloc");
        }
        transaction.commit().await.expect("commit failed");

        assert_eq!(0, *allocator.get_owner_allocated_bytes().entry(STORE_OBJECT_ID).or_default());

        // Allocate some more stuff. Due to storage pressure, this requires us to flush device
        // before the above extents are freed.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let target_len = 1500 * fs.block_size();
        while device_ranges.iter().map(|i| i.len()).sum::<u64>() != target_len {
            let len = target_len - device_ranges.iter().map(|i| i.len()).sum::<u64>();
            device_ranges.push(
                allocator
                    .allocate(&mut transaction, STORE_OBJECT_ID, len)
                    .await
                    .expect("allocate failed"),
            );
        }
        transaction.commit().await.expect("commit failed");

        assert_eq!(
            fs.block_size() * 1500,
            *allocator.get_owner_allocated_bytes().entry(STORE_OBJECT_ID).or_default()
        );
    }

    #[fuchsia::test]
    async fn test_flush() {
        const STORE_OBJECT_ID: u64 = 99;

        let mut device_ranges = Vec::new();
        let device = {
            let (fs, allocator) = test_fs().await;
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new failed");
            for _ in 0..3 {
                device_ranges.push(
                    allocator
                        .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                        .await
                        .expect("allocate failed"),
                );
            }
            transaction.commit().await.expect("commit failed");

            allocator.flush().await.expect("flush failed");

            fs.close().await.expect("close failed");
            fs.take_device().await
        };

        device.reopen(false);
        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        let allocator = fs.allocator();

        let allocated = collect_allocations(&allocator).await;

        // Make sure the ranges we allocated earlier are still allocated.
        for i in &device_ranges {
            let mut overlapping = 0;
            for j in &allocated {
                overlapping += overlap(i, j);
            }
            assert_eq!(overlapping, i.len(), "Range {i:?} not allocated");
        }

        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");

        // Make sure the extent just minted is new.
        for r in &allocated {
            assert!(r.extent < range.extent, "{r:?} isn't before {range:?}");
        }
        transaction.commit().await.expect("commit failed");
    }

    #[fuchsia::test]
    async fn test_dropped_transaction() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let allocated_range = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed")
        };
        // After dropping the transaction and attempting to allocate again, we should end up with
        // as much, because the reservation should have been released, in the next extent: the
        // dropped transaction's is dead.
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new_transaction failed");
        let range = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");
        assert_eq!(
            range,
            ExtentRange::new(
                ExtentId::new(DEFAULT_BRANCH, allocated_range.extent.sequence + 1),
                allocated_range.range
            )
        );
        assert_eq!(allocator.dead_extents(), vec![allocated_range.extent]);
    }

    #[fuchsia::test]
    async fn test_allocated_bytes() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;

        let initial_allocated_bytes = allocator.get_allocated_bytes();

        // Verify allocated_bytes reflects allocation changes.
        let allocated_bytes = initial_allocated_bytes + fs.block_size();
        let allocated_range = {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            let range = allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed");
            transaction.commit().await.expect("commit failed");
            assert_eq!(allocator.get_allocated_bytes(), allocated_bytes);
            range
        };

        {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            allocator
                .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                .await
                .expect("allocate failed");

            // Prior to committing, the count of allocated bytes shouldn't change.
            assert_eq!(allocator.get_allocated_bytes(), allocated_bytes);
        }

        // After dropping the prior transaction, the allocated bytes still shouldn't have changed.
        assert_eq!(allocator.get_allocated_bytes(), allocated_bytes);

        // Verify allocated_bytes reflects deallocations.
        let deallocate_range = ExtentRange::new(
            allocated_range.extent,
            allocated_range.range.start + 20..allocated_range.range.end - 20,
        );
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, deallocate_range)
            .await
            .expect("deallocate failed");

        // Before committing, there should be no change.
        assert_eq!(allocator.get_allocated_bytes(), allocated_bytes);

        transaction.commit().await.expect("commit failed");

        // After committing, all but 40 bytes should remain allocated.
        assert_eq!(allocator.get_allocated_bytes(), initial_allocated_bytes + 40);
    }

    #[fuchsia::test]
    async fn test_cleanup_removed_owner() {
        const STORE_OBJECT_ID: u64 = 99;
        let device = {
            let (fs, allocator) = test_fs().await;

            assert!(!allocator.get_owner_allocated_bytes().contains_key(&STORE_OBJECT_ID));
            {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .unwrap();
                allocator
                    .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
                    .await
                    .expect("Allocating")
                    .range;
                transaction.commit().await.expect("Committing.");
            }
            allocator.flush().await.expect("Flushing");
            assert!(allocator.get_owner_allocated_bytes().contains_key(&STORE_OBJECT_ID));
            {
                let mut transaction = fs
                    .root_store()
                    .new_transaction(lock_keys![], Options::default())
                    .await
                    .unwrap();
                allocator.mark_for_deletion(&mut transaction, STORE_OBJECT_ID);
                transaction.commit().await.expect("Committing.");
            }
            assert!(!allocator.get_owner_allocated_bytes().contains_key(&STORE_OBJECT_ID));
            fs.close().await.expect("Closing");
            fs.take_device().await
        };

        device.reopen(false);
        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        let allocator = fs.allocator();
        assert!(!allocator.get_owner_allocated_bytes().contains_key(&STORE_OBJECT_ID));
    }

    #[fuchsia::test]
    async fn test_persist_bytes_limit() {
        const LIMIT: u64 = 12345;
        const OWNER_ID: u64 = 12;

        let (fs, allocator) = test_fs().await;
        {
            let mut transaction = fs
                .root_store()
                .new_transaction(lock_keys![], Options::default())
                .await
                .expect("new_transaction failed");
            allocator
                .set_bytes_limit(&mut transaction, OWNER_ID, LIMIT)
                .expect("Failed to set limit.");
            assert!(allocator.inner.lock().info.limit_bytes.get(&OWNER_ID).is_none());
            transaction.commit().await.expect("Failed to commit transaction");
            let bytes: u64 = *allocator
                .inner
                .lock()
                .info
                .limit_bytes
                .get(&OWNER_ID)
                .expect("Failed to find limit");
            assert_eq!(LIMIT, bytes);
        }
    }

    #[fuchsia::test]
    async fn test_allocation_with_reservation_not_multiple_of_block_size() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;

        // Reserve an amount that isn't a multiple of the block size.
        const RESERVATION_AMOUNT: u64 = TRANSACTION_METADATA_MAX_AMOUNT + 5000;
        let reservation =
            allocator.clone().reserve(Some(STORE_OBJECT_ID), RESERVATION_AMOUNT).unwrap();

        let mut transaction = fs
            .root_store()
            .new_transaction(
                lock_keys![],
                Options { allocator_reservation: Some(&reservation), ..Options::default() },
            )
            .await
            .expect("new failed");

        let range = allocator
            .allocate(
                &mut transaction,
                STORE_OBJECT_ID,
                fs.block_size().align_up(RESERVATION_AMOUNT).unwrap(),
            )
            .await
            .expect("allocate faiiled")
            .range;
        assert!(fs.block_size().is_aligned(range.end - range.start));

        println!("{}", range.end - range.start);
    }

    /// fxdax: extents that die in transactions the journal holds, whose frees a crash may have
    /// lost, are freed after replay, with the next flush.
    #[fuchsia::test]
    async fn test_replay_frees_dead_extents() {
        const STORE_OBJECT_ID: u64 = 99;
        let fail_frees = Arc::new(AtomicBool::new(false));
        let mut device = FakeDevice::new(4096, 4096);
        device.set_op_callback({
            let fail_frees = fail_frees.clone();
            move |op| {
                if matches!(op, Op::Free) && fail_frees.load(Ordering::Relaxed) {
                    anyhow::bail!("freeing fails");
                }
                Ok(())
            }
        });
        let fs = FxFilesystem::new_empty(DeviceHolder::new(device)).await.expect("new failed");
        let allocator = fs.allocator();
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");
        transaction.commit().await.expect("commit failed");
        let device = fs.device();
        {
            let buffer = device.allocate_buffer(fs.block_size().get() as usize).await;
            device.write(range.extent, 0, buffer.as_ref()).await.expect("write failed");
        }

        // The deallocation is flushed, but the free fails, as if a crash came first.
        fail_frees.store(true, Ordering::Relaxed);
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, range.clone())
            .await
            .expect("deallocate failed");
        transaction.commit().await.expect("commit failed");
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect_err("freeing succeeded");
        assert_eq!(allocator.dead_extents(), vec![range.extent]);
        assert!(device_holds(&device, range.extent).await);
        let snapshot = device.snapshot().expect("snapshot failed");
        drop(device);
        fail_frees.store(false, Ordering::Relaxed);
        fs.close().await.expect("close failed");

        // The extent dies in replay, which found its deallocation on the device, and is freed.
        let fs = FxFilesystemBuilder::new().open(snapshot).await.expect("open failed");
        assert_eq!(fs.allocator().dead_extents(), vec![]);
        assert!(!device_holds(&fs.device(), range.extent).await);
        fs.close().await.expect("close failed");
    }

    /// fxdax: a sync that frees in the background returns with the extents its flush let die
    /// still dead; the next flush waits for their frees.
    #[fuchsia::test]
    async fn test_background_frees_finish_before_the_next_flush() {
        const STORE_OBJECT_ID: u64 = 99;
        let (fs, allocator) = test_fs().await;
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = allocator
            .allocate(&mut transaction, STORE_OBJECT_ID, fs.block_size().get())
            .await
            .expect("allocate failed");
        transaction.commit().await.expect("commit failed");
        let device = fs.device();
        {
            let buffer = device.allocate_buffer(fs.block_size().get() as usize).await;
            device.write(range.extent, 0, buffer.as_ref()).await.expect("write failed");
        }
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        allocator
            .deallocate(&mut transaction, STORE_OBJECT_ID, range.clone())
            .await
            .expect("deallocate failed");
        transaction.commit().await.expect("commit failed");

        let background =
            || SyncOptions { flush_device: true, background_frees: true, ..Default::default() };
        fs.sync(background()).await.expect("sync failed");
        assert_eq!(allocator.dead_extents(), vec![range.extent]);
        assert!(device_holds(&device, range.extent).await);

        fs.sync(background()).await.expect("sync failed");
        assert_eq!(allocator.dead_extents(), vec![]);
        assert!(!device_holds(&device, range.extent).await);
        drop(device);
        fs.close().await.expect("close failed");
    }

    /// fxdax: the allocator's info holds the extents dying, however many: more than a record of the
    /// default size holds, as removing many files at once leaves.
    #[fuchsia::test]
    fn test_info_with_many_extents_dying() {
        use crate::serialized_types::VersionedLatest;
        let info = super::AllocatorInfo {
            dying: (0..5000).map(|i| extent(1 << 40 | i)).collect(),
            ..Default::default()
        };
        let mut serialized = Vec::new();
        info.serialize_with_version(&mut serialized).expect("serialize failed");
        let (read, _version) =
            super::AllocatorInfo::deserialize_with_version(&mut std::io::Cursor::new(serialized))
                .expect("deserialize failed");
        assert_eq!(read.dying, info.dying);
    }

    /// fxdax: a crash may leave extents written whose transactions were lost: they are freed after
    /// replay, and their sequence numbers aren't minted again.
    #[fuchsia::test]
    async fn test_orphans_are_freed() {
        let (fs, allocator) = test_fs().await;
        let next_sequence = allocator.inner.lock().next_sequence;
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let orphans = [extent(next_sequence), extent(next_sequence + 2)];
        let buffer = device.allocate_buffer(device.block_size() as usize).await;
        for orphan in orphans {
            device.write(orphan, 0, buffer.as_ref()).await.expect("write failed");
        }
        drop(buffer);

        // The orphans die, with the sequence numbers between them, which the device may hold too,
        // and are freed with the next flush.
        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        assert_eq!(
            fs.allocator().dead_extents(),
            vec![extent(next_sequence), extent(next_sequence + 1), extent(next_sequence + 2)]
        );
        let range = commit_allocation(&fs).await;
        assert_eq!(range.extent, extent(next_sequence + 3));
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
        for orphan in orphans {
            assert!(!device_holds(&fs.device(), orphan).await, "{orphan:?} isn't freed");
        }
        fs.close().await.expect("close failed");
    }

    /// A transaction making a shared allocation of a block for a store, and the allocation.
    async fn shared_allocation(fs: &OpenFxFilesystem) -> (Transaction<'_>, ExtentRange) {
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        let range = fs
            .allocator()
            .allocate_shared(&mut transaction, 99, fs.block_size().get())
            .await
            .expect("allocate failed");
        (transaction, range)
    }

    /// Commits a transaction deallocating `range` of a store.
    async fn commit_deallocation(fs: &OpenFxFilesystem, range: ExtentRange) {
        let mut transaction = fs
            .root_store()
            .new_transaction(lock_keys![], Options::default())
            .await
            .expect("new failed");
        fs.allocator().deallocate(&mut transaction, 99, range).await.expect("deallocate failed");
        transaction.commit().await.expect("commit failed");
    }

    async fn flush_device(fs: &OpenFxFilesystem) {
        fs.sync(SyncOptions { flush_device: true, ..Default::default() })
            .await
            .expect("sync failed");
    }

    /// fxdax: shared allocations take the staging extent's blocks in turn, open on the device, until
    /// it is sealed; it outlives its allocations while it is the staging extent, and dies once it
    /// isn't, and is freed.
    #[fuchsia::test]
    async fn test_shared_allocations() {
        let (fs, allocator) = test_fs().await;
        let bs = fs.block_size().get();
        let (transaction, first) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        let (transaction, second) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        let staging = first.extent;
        assert_eq!(first, ExtentRange::new(staging, 0..bs));
        assert_eq!(second, ExtentRange::new(staging, bs..2 * bs));
        assert!(fs.device().is_open(staging));
        assert_eq!(allocator.extent_size(staging), Some(STAGING_SIZE));
        assert_eq!(allocator.extents()[&staging], (99, 2 * bs));

        commit_deallocation(&fs, first).await;
        commit_deallocation(&fs, second).await;
        assert_eq!(allocator.extents()[&staging], (99, 0));
        assert!(allocator.dead_extents().is_empty());

        // Sealed, it is staging no more, and dies with the next shared allocation.
        flush_device(&fs).await;
        assert!(!fs.device().is_open(staging));
        let (transaction, third) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        assert_eq!(
            third,
            ExtentRange::new(ExtentId::new(DEFAULT_BRANCH, staging.sequence + 1), 0..bs)
        );
        assert_eq!(allocator.extent_size(staging), None);
        assert_eq!(allocator.dead_extents(), vec![staging]);
        flush_device(&fs).await;
        assert!(allocator.dead_extents().is_empty());
        fs.close().await.expect("close failed");
    }

    /// fxdax: a staging extent sealed since blocks were taken of it is replaced at once, ahead of
    /// the next shared allocation, which takes it; one unused, or open still, is not, and an unused
    /// one sealed dies when it is replaced.
    #[fuchsia::test]
    async fn test_restage() {
        let (fs, allocator) = test_fs().await;
        let bs = fs.block_size().get();
        let (transaction, first) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        assert_eq!(allocator.restage(), None, "it is open");
        flush_device(&fs).await;

        let spare = allocator.restage().expect("no spare");
        assert_eq!(spare, ExtentId::new(DEFAULT_BRANCH, first.extent.sequence + 1));
        assert!(fs.device().is_open(spare));
        assert_eq!(allocator.extent_size(spare), Some(STAGING_SIZE));
        assert_eq!(allocator.restage(), None, "the spare is unused");
        let (transaction, second) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        assert_eq!(second, ExtentRange::new(spare, 0..bs));

        // Unused, a spare sealed is replaced by the next shared allocation, and dies.
        flush_device(&fs).await;
        let unused = allocator.restage().expect("no spare");
        // A barrier with nothing new in the journal, which seals the spare.
        fs.device().flush().await.expect("flush failed");
        assert!(!fs.device().is_open(unused));
        assert_eq!(allocator.restage(), None, "the sealed spare is unused");
        let (transaction, third) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        assert_eq!(third.extent, ExtentId::new(DEFAULT_BRANCH, unused.sequence + 1));
        assert_eq!(allocator.extent_size(unused), None);
        fs.close().await.expect("close failed");
    }

    /// fxdax: a shared extent doesn't die while a transaction not yet committed has allocated in
    /// it, nor with a transaction that drops: it dies once neither it nor staging pins it.
    #[fuchsia::test]
    async fn test_shared_allocations_pin() {
        let (fs, allocator) = test_fs().await;
        let bs = fs.block_size().get();
        let (transaction, first) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        let staging = first.extent;
        // Dropped, its allocation leaves a dead block.
        {
            let (_transaction, dropped) = shared_allocation(&fs).await;
            assert_eq!(dropped, ExtentRange::new(staging, bs..2 * bs));
        }
        assert_eq!(allocator.extents()[&staging], (99, bs));
        assert!(allocator.dead_extents().is_empty());

        let (pending, second) = shared_allocation(&fs).await;
        assert_eq!(second, ExtentRange::new(staging, 2 * bs..3 * bs));
        flush_device(&fs).await;
        commit_deallocation(&fs, first).await;
        // Staging moves on, but the transaction still pins it.
        let (transaction, _) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        assert_eq!(allocator.extents()[&staging], (99, 0));
        assert!(allocator.dead_extents().is_empty());
        drop(pending);
        assert!(!allocator.extents().contains_key(&staging));
        assert_eq!(allocator.dead_extents(), vec![staging]);
        fs.close().await.expect("close failed");
    }

    /// fxdax: a shared extent whose allocations all go while a transaction not yet committed has
    /// allocated in it lives on, live and in replay, which kills it with the deallocation, and
    /// revives it with the allocation: replay doesn't free it.
    #[fuchsia::test]
    async fn test_shared_allocations_pin_replayed() {
        let (fs, allocator) = test_fs().await;
        let bs = fs.block_size().get();
        let (transaction, first) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        let staging = first.extent;
        let (pending, second) = shared_allocation(&fs).await;
        commit_deallocation(&fs, first).await;
        pending.commit().await.expect("commit failed");
        assert_eq!(allocator.extents()[&staging], (99, bs));
        let device = fs.device();
        {
            let mut buffer = device.allocate_buffer(bs as usize).await;
            buffer.as_mut().fill(5);
            device.write(staging, second.range.start, buffer.as_ref()).await.expect("write");
        }
        // The journal on the device, as a crash would leave it, for replay.
        flush_device(&fs).await;
        let snapshot = device.snapshot().expect("snapshot failed");
        drop(device);
        fs.close().await.expect("close failed");

        let fs = FxFilesystemBuilder::new().open(snapshot).await.expect("open failed");
        let allocator = fs.allocator();
        assert_eq!(allocator.extents()[&staging], (99, bs));
        assert!(allocator.dead_extents().is_empty());
        flush_device(&fs).await;
        assert!(device_holds(&fs.device(), staging).await);
        fs.close().await.expect("close failed");
    }

    /// fxdax: a shared extent is alive, after a remount, until all its allocations are
    /// deallocated, and is checked as any other.
    #[fuchsia::test]
    async fn test_shared_allocations_remount() {
        let (fs, _) = test_fs().await;
        let bs = fs.block_size().get();
        let (transaction, first) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        let (transaction, second) = shared_allocation(&fs).await;
        transaction.commit().await.expect("commit failed");
        fs.close().await.expect("close failed");
        let device = fs.take_device().await;
        device.reopen(false);
        let fs = FxFilesystemBuilder::new().open(device).await.expect("open failed");
        let allocator = fs.allocator();
        let staging = first.extent;
        assert_eq!(allocator.extents()[&staging], (99, 2 * bs));
        // It is staging no more.
        assert_eq!(allocator.extent_size(staging), Some(2 * bs));

        commit_deallocation(&fs, second).await;
        assert_eq!(allocator.extents()[&staging], (99, bs));
        commit_deallocation(&fs, first).await;
        assert_eq!(allocator.dead_extents(), vec![staging]);
        fs.close().await.expect("close failed");
    }
}
