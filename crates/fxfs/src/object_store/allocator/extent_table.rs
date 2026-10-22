// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: the lives of the allocator's extents, from their minting to the seal of their
//! frees (backlog doc-4; the allocator's "Dead extents", "Shared extents" and "Orphaned extents").
//!
//! [`ExtentTable`] knows each extent the allocator minted or found in its tree, in one of these
//! states:
//!
//! - Minted for an allocation that isn't shared, whose transaction hasn't committed: the table
//!   doesn't hold it until it does. If the transaction drops, the extent dies, as far as it was
//!   allocated, as its data may have been written (`drop_allocation`).
//! - Pending ([`ExtentState::Pending`]): minted for shared allocations, none of which has
//!   committed: the staging extent, or one transactions not yet committed have allocated in. Each
//!   of those pins it, as being the staging extent does. Once the last pin goes, it dies, taking
//!   nothing, as nothing records it.
//! - Allocated ([`ExtentState::Allocated`]): alive, with some of it allocated. It reaches as far as
//!   its last range, allocated or not, and each range allocated is referenced by one file's
//!   extents, or by more (reflinks). It dies once the last of it is deallocated, unless pins hold
//!   it, as a shared one's still may; and once its owner is deleted, whatever holds it.
//! - Empty ([`ExtentState::Empty`]): alive, shared, with nothing of it allocated, as pins hold it.
//!   It dies once the last goes, to be freed as it would have been when it emptied; it is
//!   allocated again if an allocation in it commits first, and it dies with its owner too.
//! - Dead: queued, in the order extents died, until its [`FreeAfter`] allows its free.
//! - Being freed: out of the table, in a [`Freeing`] with others, while the device frees them; one
//!   the device doesn't free is dead again, at the back of the queue.
//! - Freed, whose free a crash might undo until a device flush that began after it is done
//!   (`seal_begins`, `sealed`): the allocator's info holds it as dying meanwhile.
//!
//! Each transition checks first, and changes nothing if a check fails. Each keeps two totals: the
//! bytes the extents alive take, whole, and those the dead take, with those being freed, which the
//! device holds until they are. And each returns the records it implies for the allocator's tree
//! ([`Records`]), which the allocator merges into it: an allocation's range, counted once
//! (`commit`); the parts of a range whose references change, with their new counts, a part none
//! references any more counted 0, so that its extent's size is known while the rest is alive
//! (`recount`); and an extent that dies, which leaves the tree, once it does so with its last
//! deallocation, or as an empty one with its last pin, which no transaction journals (`restage`,
//! `drop_allocation`). The other deaths write nothing: those of owners deleted, whose records a
//! compaction filters out; of extents the tree has no records of (those transactions that dropped
//! allocated, orphans, and those dying when it was last compacted); and of those a mount finds with
//! nothing allocated, whose ranges counted 0 stay in the tree.
//!
//! Replay applies the transactions the journal holds, which pins aren't in: there, an extent dies
//! with its last deallocation, to be freed once replay is done. An allocation replayed in an extent
//! that died in this replay, and isn't yet freed, revives it, as one of a shared extent that pins
//! kept alive, live.

use crate::errors::FxfsError;
use crate::filesystem::{ApplyContext, ApplyMode};
use crate::log::*;
use crate::object_handle::INVALID_OBJECT_ID;
use crate::object_store::allocator::{
    AllocatorItem, AllocatorKey, AllocatorValue, RangeCounts, STAGED_MAX, STAGING_SIZE,
};
use crate::range::RangeExt;
use anyhow::{Error, anyhow, bail, ensure};
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZero;
use std::ops::Range;
use storage_device::{Device, ExtentId, ExtentRange};

/// The allocator's extents, from their minting to the seal of their frees (see the module's doc).
#[derive(Default)]
pub(super) struct ExtentTable {
    /// The extents alive, and those pending.
    extents: HashMap<ExtentId, ExtentState>,
    /// The bytes the extents alive take, whole.
    alive_bytes: u64,
    /// The staging extent, if there is one.
    staging: Option<Staging>,
    /// The extents that died and are not yet freed, nor being freed, in the order they died.
    dead: VecDeque<DeadExtent>,
    /// The bytes the dead extents take, with those being freed.
    dead_bytes: u64,
    /// The extents freed since the last device flush began, whose frees a crash might undo.
    unsealed: Vec<ExtentId>,
    /// The extents freed before the device flush in progress began, whose frees it seals.
    sealing: Vec<ExtentId>,
    /// The sequence number of the next extent minted.
    next_sequence: u64,
}

/// The state of an extent the table holds, which hasn't died.
#[derive(Debug)]
enum ExtentState {
    /// Shared, with no allocation in it committed yet. `pins` hold it: one while it is the staging
    /// extent, and one for each allocation in it of a transaction not yet committed.
    Pending { pins: NonZero<usize> },
    /// Alive, with some of it allocated; and, if it is shared, held by `pins`, as above.
    Allocated { info: ExtentInfo, pins: usize },
    /// Alive, and shared, with nothing of it allocated, as `pins` hold it. It dies once they go, to
    /// be freed once `emptied` allows: as it would have been when the last of it was deallocated.
    Empty { owner_object_id: u64, size: u64, pins: NonZero<usize>, emptied: FreeAfter },
}

impl ExtentState {
    /// Whether the extent is alive: allocated, or empty.
    fn is_alive(&self) -> bool {
        !matches!(self, ExtentState::Pending { .. })
    }

    /// The owner the extent's allocations are recorded under, if it is alive.
    fn owner(&self) -> Option<u64> {
        match self {
            ExtentState::Pending { .. } => None,
            ExtentState::Allocated { info, .. } => Some(info.owner_object_id),
            ExtentState::Empty { owner_object_id, .. } => Some(*owner_object_id),
        }
    }

    /// The bytes the extent takes: as far as its last range, allocated or not; pending, none.
    fn size(&self) -> u64 {
        match self {
            ExtentState::Pending { .. } => 0,
            ExtentState::Allocated { info, .. } => info.size,
            ExtentState::Empty { size, .. } => *size,
        }
    }

    /// The bytes of the extent allocated.
    fn allocated(&self) -> u64 {
        match self {
            ExtentState::Allocated { info, .. } => info.allocated.get(),
            ExtentState::Pending { .. } | ExtentState::Empty { .. } => 0,
        }
    }

    /// How many pins hold the extent.
    fn pins(&self) -> usize {
        match self {
            ExtentState::Pending { pins } | ExtentState::Empty { pins, .. } => pins.get(),
            ExtentState::Allocated { pins, .. } => *pins,
        }
    }

    /// Adds a pin.
    fn pin(&mut self) {
        match self {
            ExtentState::Pending { pins } | ExtentState::Empty { pins, .. } => {
                *pins = pins.saturating_add(1)
            }
            ExtentState::Allocated { pins, .. } => *pins += 1,
        }
    }
}

/// An extent alive with some of it allocated.
#[derive(Debug)]
struct ExtentInfo {
    /// The owner its allocations are recorded under.
    owner_object_id: u64,
    /// Its size: where its last range ends, allocated or not.
    size: u64,
    /// The bytes of it still allocated.
    allocated: NonZero<u64>,
    /// Its ranges more than one file's extents reference (reflinks, "References"), with how many:
    /// the rest of what is allocated, once.
    references: RangeCounts,
}

impl ExtentInfo {
    /// The parts of `range`, which is allocated, with how many references each has.
    fn references(&self, range: Range<u64>) -> Vec<(Range<u64>, u64)> {
        let mut parts = self.references.counts(range);
        for (_, count) in &mut parts {
            *count = (*count).max(1);
        }
        parts
    }
}

/// The staging extent, which shared allocations take blocks of.
#[derive(Debug)]
struct Staging {
    extent: ExtentId,
    /// The data owner its allocations are recorded under.
    owner_object_id: u64,
    /// Where its next allocation starts.
    next: u64,
}

/// An extent that died, which is freed once `free_after` allows.
#[derive(Debug)]
struct DeadExtent {
    free_after: FreeAfter,
    extent: ExtentId,
    size: u64,
    owner_object_id: u64,
}

/// What has to be on the device before a dead extent is freed.
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

/// How a transaction is applied, which decides when the extents that die in it can be freed.
#[derive(Clone, Copy, Debug)]
pub(super) enum Applied {
    /// Live, with the journal at this offset.
    Live(u64),
    /// In replay, which found the transaction on the device.
    Replayed,
}

impl Applied {
    /// How the transaction of `context` is applied.
    pub(super) fn of(context: &ApplyContext<'_, '_>) -> Self {
        match context.mode {
            ApplyMode::Live(_) => Applied::Live(context.checkpoint.file_offset),
            ApplyMode::Replay => Applied::Replayed,
        }
    }

    /// When an extent that dies in the transaction can be freed: live, once the device is flushed
    /// past the transaction, and in replay, at once; with a record of the flush if `recorded`,
    /// which any flush has, in replay.
    fn free_after(self, recorded: bool) -> FreeAfter {
        match (self, recorded) {
            (Applied::Live(offset), false) => FreeAfter::Flush(offset),
            (Applied::Live(offset), true) => FreeAfter::RecordedFlush(offset),
            (Applied::Replayed, false) => FreeAfter::Now,
            (Applied::Replayed, true) => FreeAfter::RecordedFlush(0),
        }
    }
}

/// The records of the allocator's tree a transition implies, in the order the allocator merges
/// them into it.
#[must_use]
#[derive(Debug, Default)]
pub(super) struct Records(Vec<AllocatorItem>);

impl Records {
    /// Adds the record of `range` of `extent`, referenced `count` times, for `owner_object_id`.
    fn add_count(&mut self, extent: ExtentId, range: Range<u64>, count: u64, owner_object_id: u64) {
        let value = AllocatorValue::Abs { count, owner_object_id };
        self.0.push(AllocatorItem { key: AllocatorKey::new(extent, range), value });
    }

    /// Adds the removal of `extent`, of `size` bytes, which died, from the tree.
    fn add_removal(&mut self, extent: ExtentId, size: u64) {
        let key = AllocatorKey::new(extent, 0..size);
        self.0.push(AllocatorItem { key, value: AllocatorValue::None });
    }
}

impl IntoIterator for Records {
    type Item = AllocatorItem;
    type IntoIter = std::vec::IntoIter<AllocatorItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// What a change of a range's references did (`ExtentTable::recount`).
#[must_use]
#[derive(Debug)]
pub(super) struct Recount {
    pub(super) records: Records,
    /// The owner the range's extent is recorded under.
    pub(super) owner_object_id: u64,
    /// The bytes of the range deallocated: those that lost their last reference.
    pub(super) deallocated: u64,
    /// The size of the range's extent, if it died.
    pub(super) died: Option<u64>,
}

/// The extents the allocator's tree holds, as a mount finds them (`ExtentTable::load`).
#[derive(Default)]
pub(super) struct Loading(HashMap<ExtentId, Loaded>);

/// An extent the allocator's tree holds, as far as a mount has found it.
struct Loaded {
    owner_object_id: u64,
    size: u64,
    allocated: u64,
    references: RangeCounts,
}

impl Loading {
    /// Adds the tree's record of `key`, referenced `count` times, for `owner_object_id`.
    pub(super) fn add(
        &mut self,
        key: &AllocatorKey,
        count: u64,
        owner_object_id: u64,
    ) -> Result<(), Error> {
        let extent = self.0.entry(key.extent).or_insert(Loaded {
            owner_object_id,
            size: 0,
            allocated: 0,
            references: RangeCounts::default(),
        });
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
        Ok(())
    }
}

/// Dead extents being freed, out of the table, in the order they died
/// (`ExtentTable::start_freeing`), until their frees' results come back
/// (`ExtentTable::finish_freeing`).
#[must_use]
pub(super) struct Freeing(Vec<DeadExtent>);

impl Freeing {
    /// The extents to free, in order.
    pub(super) fn extents(&self) -> Vec<ExtentId> {
        self.0.iter().map(|dead| dead.extent).collect()
    }
}

impl ExtentTable {
    /// Mints an extent of `branch`, opened on `device`: open until a seal, as none can have sealed
    /// an extent not yet minted.
    pub(super) fn mint(&mut self, branch: u64, device: &dyn Device) -> ExtentId {
        let extent = ExtentId::new(branch, self.next_sequence);
        self.next_sequence += 1;
        device.opened(extent);
        extent
    }

    /// Mints from `sequence` on.
    pub(super) fn mint_from(&mut self, sequence: u64) {
        self.next_sequence = sequence;
    }

    /// Takes `len` bytes, at most STAGED_MAX, of the staging extent of `owner_object_id`, a data
    /// owner, for a transaction not yet committed, which pins it. Unless that is open on `device`,
    /// with room, a new one of `branch` takes its place, opened on the device.
    pub(super) fn stage(
        &mut self,
        branch: u64,
        owner_object_id: u64,
        len: u64,
        device: &dyn Device,
    ) -> (ExtentRange, Records) {
        debug_assert!(len <= STAGED_MAX);
        let fits = self.staging.as_ref().is_some_and(|staging| {
            staging.owner_object_id == owner_object_id
                && staging.next + len <= STAGING_SIZE
                && device.is_open(staging.extent)
        });
        let records = if fits {
            Records::default()
        } else {
            self.replace_staging(branch, owner_object_id, device).1
        };
        let staging = self.staging.as_mut().unwrap();
        let range = ExtentRange::new(staging.extent, staging.next..staging.next + len);
        staging.next += len;
        self.extents.get_mut(&range.extent).expect("the staging extent is held").pin();
        (range, records)
    }

    /// Replaces the staging extent, if a barrier sealed it since blocks were taken of it, with a
    /// new one of `branch`, opened on `device`, for the same owner, and returns it.
    pub(super) fn restage(
        &mut self,
        branch: u64,
        device: &dyn Device,
    ) -> Option<(ExtentId, Records)> {
        let staging = self.staging.as_ref()?;
        if staging.next == 0 || device.is_open(staging.extent) {
            return None;
        }
        let owner_object_id = staging.owner_object_id;
        Some(self.replace_staging(branch, owner_object_id, device))
    }

    /// Makes a new extent of `branch`, opened on `device`, the staging extent of
    /// `owner_object_id`, in place of the last, which it no longer pins.
    fn replace_staging(
        &mut self,
        branch: u64,
        owner_object_id: u64,
        device: &dyn Device,
    ) -> (ExtentId, Records) {
        let records = match self.staging.take() {
            Some(old) => self.unpin(old.extent),
            None => Records::default(),
        };
        let extent = self.mint(branch, device);
        self.extents.insert(extent, ExtentState::Pending { pins: NonZero::<usize>::MIN });
        self.staging = Some(Staging { extent, owner_object_id, next: 0 });
        (extent, records)
    }

    /// Commits the allocation of `range` for `owner_object_id`, in a transaction `applied`, and
    /// returns its record. Its extent is alive from then on, as far as the range at least, and
    /// recorded under that owner; a live allocation in a shared extent no longer pins it.
    pub(super) fn commit(
        &mut self,
        range: &ExtentRange,
        owner_object_id: u64,
        applied: Applied,
    ) -> Result<Records, Error> {
        let extent = range.extent;
        let Some(len) = NonZero::new(range.len()) else {
            bail!(anyhow!(FxfsError::Inconsistent).context(format!("{range:?} allocated empty")));
        };
        if let Some(owner) = self.owner(extent) {
            ensure!(
                owner == owner_object_id,
                anyhow!(FxfsError::Inconsistent).context(format!(
                    "{range:?} allocated for {owner_object_id}, but its extent is {owner}'s"
                ))
            );
        }
        if let Applied::Replayed = applied {
            self.revive(extent);
        }
        let new = |size| ExtentInfo {
            owner_object_id,
            size,
            allocated: len,
            references: RangeCounts::default(),
        };
        let (mut info, mut pins) = match self.extents.remove(&extent) {
            None => (new(0), 0),
            Some(ExtentState::Pending { pins }) => (new(0), pins.get()),
            Some(ExtentState::Empty { size, pins, .. }) => (new(size), pins.get()),
            Some(ExtentState::Allocated { mut info, pins }) => {
                info.allocated = info.allocated.saturating_add(len.get());
                (info, pins)
            }
        };
        if range.range.end > info.size {
            self.alive_bytes += range.range.end - info.size;
            info.size = range.range.end;
        }
        if let Applied::Live(_) = applied {
            pins = pins.saturating_sub(1);
        }
        self.extents.insert(extent, ExtentState::Allocated { info, pins });
        let mut records = Records::default();
        records.add_count(extent, range.range.clone(), 1, owner_object_id);
        Ok(records)
    }

    /// Revives `extent`, if it died in this replay but isn't yet freed, which an allocation
    /// replayed is in: as one of a shared extent kept alive, live, by its pins.
    fn revive(&mut self, extent: ExtentId) {
        if self.is_alive(extent) {
            return;
        }
        if let Some(index) = self.dead.iter().position(|dead| dead.extent == extent) {
            let dead = self.dead.remove(index).unwrap();
            self.dead_bytes -= dead.size;
        }
    }

    /// Changes how many times each part of `range`, which is allocated, is referenced by `delta`
    /// (see the allocator's "References"), in a transaction `applied`. A part referenced no more is
    /// deallocated; once nothing of the extent is allocated, it dies, unless pins hold it, when it
    /// is empty.
    pub(super) fn recount(
        &mut self,
        range: &ExtentRange,
        delta: i64,
        applied: Applied,
    ) -> Result<Recount, Error> {
        let extent = range.extent;
        let Some(ExtentState::Allocated { info, pins }) = self.extents.get_mut(&extent) else {
            bail!(anyhow!(FxfsError::Inconsistent).context(format!("{range:?} isn't allocated")));
        };
        let mut parts = Vec::new();
        let mut deallocated = 0;
        for (part, count) in info.references(range.range.clone()) {
            let count = count.checked_add_signed(delta).ok_or_else(|| {
                anyhow!(FxfsError::Inconsistent).context(format!(
                    "{part:?} of {extent:?} referenced {delta} times more than {count}"
                ))
            })?;
            if count == 0 {
                ensure!(
                    part.end <= info.size,
                    anyhow!(FxfsError::Inconsistent)
                        .context(format!("{range:?} isn't allocated in {info:?}"))
                );
                deallocated += part.end - part.start;
            }
            parts.push((part, count));
        }
        // Nothing of an extent can be referenced once nothing of it is allocated.
        let allocated = info.allocated.get().checked_sub(deallocated);
        ensure!(
            allocated.is_some_and(|left| left > 0 || parts.iter().all(|(_, count)| *count == 0)),
            anyhow!(FxfsError::Inconsistent)
                .context(format!("{range:?} isn't allocated in {info:?}"))
        );

        let owner_object_id = info.owner_object_id;
        let mut records = Records::default();
        for (part, count) in parts {
            if count > 0 {
                info.references.set(part.clone(), if count > 1 { count } else { 0 });
            }
            records.add_count(extent, part, count, owner_object_id);
        }
        let mut died = None;
        match NonZero::new(info.allocated.get() - deallocated) {
            Some(allocated) => info.allocated = allocated,
            None => {
                let size = info.size;
                match NonZero::new(*pins) {
                    Some(pins) => {
                        let emptied = applied.free_after(false);
                        let empty = ExtentState::Empty { owner_object_id, size, pins, emptied };
                        self.extents.insert(extent, empty);
                    }
                    None => {
                        // The extent leaves the tree with its last part, which kept its ranges
                        // deallocated only so that its size was known.
                        records.0.pop();
                        records.add_removal(extent, size);
                        self.die(extent, applied.free_after(false));
                        died = Some(size);
                    }
                }
            }
        }
        Ok(Recount { records, owner_object_id, deallocated, died })
    }

    /// Drops the allocation of `range` for `owner_object_id`, of a transaction that dropped. The
    /// extent's data may have been written: it dies, as far as the range, to be freed at once,
    /// which the next flush seals. A shared extent's other allocations, and staging, keep it until
    /// they go.
    pub(super) fn drop_allocation(&mut self, range: &ExtentRange, owner_object_id: u64) -> Records {
        if self.is_pinned(range.extent) {
            return self.unpin(range.extent);
        }
        // Branch 0's extents are those no allocator mints, which aren't freed.
        if range.extent.branch != 0 {
            self.queue_dead(DeadExtent {
                free_after: FreeAfter::Now,
                extent: range.extent,
                size: range.range.end,
                owner_object_id,
            });
        }
        Records::default()
    }

    /// Drops a pin of `extent`, if pins hold it. Once none do, a pending one dies, as no
    /// transaction that allocated in it committed, and its data may have been written; and an empty
    /// one dies, and leaves the tree, as it would have when it emptied. The death isn't journaled:
    /// replay kills the extent with its last deallocation, which pins don't hold off there.
    fn unpin(&mut self, extent: ExtentId) -> Records {
        let mut records = Records::default();
        let Some(state) = self.extents.get_mut(&extent) else {
            return records;
        };
        match state {
            ExtentState::Allocated { pins, .. } => *pins = pins.saturating_sub(1),
            ExtentState::Pending { pins } | ExtentState::Empty { pins, .. } if pins.get() > 1 => {
                *pins = NonZero::new(pins.get() - 1).unwrap()
            }
            ExtentState::Pending { .. } => self.die(extent, FreeAfter::Now),
            &mut ExtentState::Empty { size, emptied, .. } => {
                self.die(extent, emptied);
                records.add_removal(extent, size);
            }
        }
        records
    }

    /// Kills the extents of `owners`, which are deleted in a transaction `applied`: they are freed
    /// once the device is flushed past it, with a record of the flush.
    pub(super) fn kill_owners(&mut self, owners: &[u64], applied: Applied) {
        let killed: Vec<_> = self
            .extents
            .iter()
            .filter(|(_, state)| state.owner().is_some_and(|owner| owners.contains(&owner)))
            .map(|(extent, _)| *extent)
            .collect();
        for extent in killed {
            self.die(extent, applied.free_after(true));
        }
    }

    /// Loads the extents of the allocator's tree, as a mount finds them, before replay: those
    /// alive, and the dead, which are those of owners marked for deletion, those of which nothing
    /// is allocated, and those `dying` when the tree was last compacted, which it doesn't hold.
    pub(super) fn load(
        &mut self,
        loading: Loading,
        dying: Vec<ExtentId>,
        marked_for_deletion: &HashSet<u64>,
    ) {
        debug_assert!(self.extents.is_empty() && self.dead.is_empty());
        for extent in dying {
            if !loading.0.contains_key(&extent) {
                self.queue_dead(DeadExtent {
                    free_after: FreeAfter::Now,
                    extent,
                    size: 0,
                    owner_object_id: INVALID_OBJECT_ID,
                });
            }
        }
        for (extent, Loaded { owner_object_id, size, allocated, references }) in loading.0 {
            if marked_for_deletion.contains(&owner_object_id) {
                let free_after = FreeAfter::RecordedFlush(0);
                self.queue_dead(DeadExtent { free_after, extent, size, owner_object_id });
            } else if let Some(allocated) = NonZero::new(allocated) {
                let info = ExtentInfo { owner_object_id, size, allocated, references };
                self.alive_bytes += size;
                self.extents.insert(extent, ExtentState::Allocated { info, pins: 0 });
            } else {
                let free_after = FreeAfter::Now;
                self.queue_dead(DeadExtent { free_after, extent, size, owner_object_id });
            }
        }
    }

    /// Kills the extents of `branch` with `sequences` that aren't alive: those a crash left, whose
    /// transactions were lost, which are freed with the next flush of the device with a record of
    /// it. Returns how many.
    pub(super) fn kill_orphans(&mut self, branch: u64, sequences: Range<u64>) -> usize {
        let orphans: Vec<_> = sequences
            .map(|sequence| ExtentId::new(branch, sequence))
            .filter(|extent| !self.is_alive(*extent))
            .collect();
        for &extent in &orphans {
            self.queue_dead(DeadExtent {
                free_after: FreeAfter::RecordedFlush(0),
                extent,
                size: 0,
                owner_object_id: INVALID_OBJECT_ID,
            });
        }
        orphans.len()
    }

    /// Kills `extent`, which the table holds, to be freed once `free_after` allows, as large as it
    /// is, and its owner's; pending, it takes nothing, and has none.
    fn die(&mut self, extent: ExtentId, free_after: FreeAfter) {
        let state = self.extents.remove(&extent).expect("an extent held dies");
        let size = state.size();
        self.alive_bytes -= size;
        if self.staging.as_ref().is_some_and(|staging| staging.extent == extent) {
            self.staging = None;
        }
        let owner_object_id = state.owner().unwrap_or(INVALID_OBJECT_ID);
        self.queue_dead(DeadExtent { free_after, extent, size, owner_object_id });
    }

    /// Queues `dead`: an extent that died, which the table no longer holds, or never did, taking
    /// as many bytes as its killer knows of.
    fn queue_dead(&mut self, dead: DeadExtent) {
        self.dead_bytes += dead.size;
        self.dead.push_back(dead);
    }

    /// Takes the dead extents that a flush of the journal to `log_file_offset`, recorded if
    /// `recorded`, allows to be freed, in the order they died, to be freed.
    pub(super) fn start_freeing(&mut self, log_file_offset: u64, recorded: bool) -> Freeing {
        let (free, wait): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.dead)
            .into_iter()
            .partition(|dead| dead.free_after.allows(log_file_offset, recorded));
        self.dead = wait;
        Freeing(free.into())
    }

    /// Takes back `freeing`, with `results`, those of the frees of its extents, in order. The
    /// extents freed are unsealed until a device flush seals their frees; those not freed are dead
    /// again, at the back of the queue, and tried again later.
    pub(super) fn finish_freeing(
        &mut self,
        freeing: Freeing,
        results: Vec<Result<(), Error>>,
    ) -> Result<(), Error> {
        let mut first_error = None;
        for (dead, result) in freeing.0.into_iter().zip(results) {
            match result {
                Ok(()) => {
                    self.dead_bytes -= dead.size;
                    self.unsealed.push(dead.extent);
                }
                Err(error) => {
                    error!(error:?, extent:? = dead.extent; "Failed to free extent");
                    first_error = Some(error);
                    self.dead.push_back(dead);
                }
            }
        }
        match first_error {
            Some(error) => Err(error.context("Failed to free dead extents")),
            None => Ok(()),
        }
    }

    /// The device begins a flush, which seals the frees made before it.
    pub(super) fn seal_begins(&mut self) {
        let unsealed = std::mem::take(&mut self.unsealed);
        self.sealing.extend(unsealed);
    }

    /// The device's flush is done, which sealed the frees made before it began.
    pub(super) fn sealed(&mut self) {
        self.sealing.clear();
    }

    /// The extents alive, with their owners and the bytes of them allocated.
    pub(super) fn alive(&self) -> impl Iterator<Item = (ExtentId, u64, u64)> + '_ {
        self.extents.iter().filter_map(|(extent, state)| {
            state.owner().map(|owner_object_id| (*extent, owner_object_id, state.allocated()))
        })
    }

    /// The extents that died and are not yet freed, nor being freed, in the order they died.
    pub(super) fn dead(&self) -> impl ExactSizeIterator<Item = ExtentId> + '_ {
        self.dead.iter().map(|dead| dead.extent)
    }

    /// The extents dead, and those freed whose frees a crash might undo; not those being freed.
    pub(super) fn dying(&self) -> Vec<ExtentId> {
        let mut dying: Vec<_> = self
            .dead()
            .chain(self.unsealed.iter().copied())
            .chain(self.sealing.iter().copied())
            .collect();
        dying.sort();
        dying.dedup();
        dying
    }

    /// The owner `extent`'s allocations are recorded under, if it is alive.
    pub(super) fn owner(&self, extent: ExtentId) -> Option<u64> {
        self.extents.get(&extent)?.owner()
    }

    /// Whether `extent` is alive.
    pub(super) fn is_alive(&self, extent: ExtentId) -> bool {
        self.extents.get(&extent).is_some_and(ExtentState::is_alive)
    }

    /// Whether pins hold `extent`: the staging extent, or one transactions not yet committed have
    /// allocated in.
    pub(super) fn is_pinned(&self, extent: ExtentId) -> bool {
        self.extents.get(&extent).is_some_and(|state| state.pins() > 0)
    }

    /// Whether some of `range` is referenced more than once (see the allocator's "References").
    pub(super) fn is_referenced_more_than_once(&self, range: &ExtentRange) -> bool {
        match self.extents.get(&range.extent) {
            Some(ExtentState::Allocated { info, .. }) => {
                info.references(range.range.clone()).iter().any(|(_, count)| *count > 1)
            }
            _ => false,
        }
    }

    /// How far `extent` reaches, if it is alive, or for the staging extent, while it is open on
    /// `device`, may yet.
    pub(super) fn extent_size(&self, extent: ExtentId, device: &dyn Device) -> Option<u64> {
        if self.staging.as_ref().is_some_and(|staging| staging.extent == extent)
            && device.is_open(extent)
        {
            return Some(STAGING_SIZE);
        }
        self.extents.get(&extent).filter(|state| state.is_alive()).map(ExtentState::size)
    }

    /// The bytes the extents alive take, whole.
    pub(super) fn alive_bytes(&self) -> u64 {
        self.alive_bytes
    }

    /// The bytes the dead extents take, with those being freed.
    pub(super) fn dead_bytes(&self) -> u64 {
        self.dead_bytes
    }

    /// Whether freeing the extents that died before `log_file_offset` needs a record of the
    /// device's flush to it (see `FreeAfter::RecordedFlush`).
    pub(super) fn needs_recorded_flush(&self, log_file_offset: u64) -> bool {
        self.dead.iter().any(|dead| match dead.free_after {
            FreeAfter::RecordedFlush(offset) => offset < log_file_offset,
            _ => false,
        })
    }

    /// Whether an extent of `owner_object_id`'s is dead; not one being freed.
    pub(super) fn holds_dead_of(&self, owner_object_id: u64) -> bool {
        self.dead.iter().any(|dead| dead.owner_object_id == owner_object_id)
    }

    /// The sequence number of the next extent minted.
    #[cfg(test)]
    pub(super) fn next_sequence(&self) -> u64 {
        self.next_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::{Applied, ExtentState, ExtentTable, FreeAfter, Loading};
    use crate::errors::FxfsError;
    use crate::object_handle::INVALID_OBJECT_ID;
    use crate::object_store::allocator::{
        AllocatorItem, AllocatorKey, AllocatorValue, STAGING_SIZE,
    };
    use anyhow::anyhow;
    use std::collections::HashSet;
    use std::ops::Range;
    use storage_device::fake_device::{DEFAULT_BRANCH, FakeDevice};
    use storage_device::{Device, ExtentId, ExtentRange};

    const BS: u64 = 4096;
    const OWNER: u64 = 99;
    const OTHER_OWNER: u64 = 100;

    impl ExtentTable {
        /// Checks the table's invariants, with no frees in progress: its byte totals are its
        /// extents', each extent's state is one it can be in, and the staging extent is pinned.
        /// Not that no extent alive is dead too: one is, if an allocation in it commits after its
        /// owner's extents were killed.
        fn check(&self) {
            let alive_bytes: u64 = self.extents.values().map(ExtentState::size).sum();
            assert_eq!(self.alive_bytes, alive_bytes, "the bytes alive");
            let dead_bytes: u64 = self.dead.iter().map(|dead| dead.size).sum();
            assert_eq!(self.dead_bytes, dead_bytes, "the bytes dead");
            for (extent, state) in &self.extents {
                match state {
                    ExtentState::Pending { .. } => {}
                    ExtentState::Allocated { info, .. } => {
                        assert!(info.allocated.get() <= info.size, "{extent:?} is {info:?}")
                    }
                    ExtentState::Empty { size, .. } => {
                        assert!(*size > 0, "{extent:?} is {state:?}")
                    }
                }
            }
            if let Some(staging) = &self.staging {
                assert!(self.is_pinned(staging.extent), "{staging:?} isn't pinned");
                assert!(staging.next <= STAGING_SIZE, "{staging:?} is past its end");
            }
        }

        /// The extents dead, in the order they died, with what has to be on the device before
        /// each is freed, and their sizes and owners.
        fn dead_extents(&self) -> Vec<(ExtentId, FreeAfter, u64, u64)> {
            self.dead
                .iter()
                .map(|dead| (dead.extent, dead.free_after, dead.size, dead.owner_object_id))
                .collect()
        }
    }

    /// The record of `range` of `extent`, referenced `count` times, for OWNER.
    fn counted(extent: ExtentId, range: Range<u64>, count: u64) -> AllocatorItem {
        let value = AllocatorValue::Abs { count, owner_object_id: OWNER };
        AllocatorItem { key: AllocatorKey::new(extent, range), value }
    }

    /// The removal of `extent`, of `size` bytes, from the tree.
    fn removed(extent: ExtentId, size: u64) -> AllocatorItem {
        AllocatorItem { key: AllocatorKey::new(extent, 0..size), value: AllocatorValue::None }
    }

    /// Commits the allocation of `range` for OWNER, which records it, once.
    fn commit(table: &mut ExtentTable, range: &ExtentRange, applied: Applied) {
        let records = table.commit(range, OWNER, applied).expect("commit failed");
        assert_eq!(records.0, [counted(range.extent, range.range.clone(), 1)]);
        table.check();
    }

    /// An extent dies with its last allocation, which removes it from the tree, and is freed once
    /// the journal is flushed past where it died.
    #[fuchsia::test]
    fn an_extent_dies_with_its_last_allocation() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        assert!(device.is_open(extent));
        commit(&mut table, &ExtentRange::new(extent, 0..2 * BS), Applied::Live(10));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, 2 * BS)]);

        let head = ExtentRange::new(extent, 0..BS);
        let recount = table.recount(&head, -1, Applied::Live(20)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(extent, 0..BS, 0)]);
        assert_eq!((recount.owner_object_id, recount.deallocated, recount.died), (OWNER, BS, None));
        // It takes its whole size until it dies.
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, BS)]);
        assert_eq!(table.alive_bytes(), 2 * BS);
        table.check();

        let tail = ExtentRange::new(extent, BS..2 * BS);
        let recount = table.recount(&tail, -1, Applied::Live(30)).expect("recount failed");
        assert_eq!(recount.records.0, [removed(extent, 2 * BS)]);
        assert_eq!((recount.deallocated, recount.died), (BS, Some(2 * BS)));
        assert!(!table.is_alive(extent));
        assert_eq!(table.dead_extents(), [(extent, FreeAfter::Flush(30), 2 * BS, OWNER)]);
        assert_eq!((table.alive_bytes(), table.dead_bytes()), (0, 2 * BS));
        table.check();

        let early = table.start_freeing(30, false);
        assert!(early.extents().is_empty());
        table.finish_freeing(early, vec![]).expect("free failed");
        let freeing = table.start_freeing(31, false);
        assert_eq!(freeing.extents(), [extent]);
        // It is counted until it is freed.
        assert_eq!(table.dead_bytes(), 2 * BS);
        table.finish_freeing(freeing, vec![Ok(())]).expect("free failed");
        assert_eq!((table.dead_bytes(), table.dying()), (0, vec![extent]));
        table.check();
    }

    /// A deallocation takes a reference from each part of its range, and deallocates those that
    /// had one.
    #[fuchsia::test]
    fn references_split_a_deallocation() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        commit(&mut table, &ExtentRange::new(extent, 0..4 * BS), Applied::Live(10));

        // A reflink of the second block.
        let second = ExtentRange::new(extent, BS..2 * BS);
        let recount = table.recount(&second, 1, Applied::Live(20)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(extent, BS..2 * BS, 2)]);
        assert_eq!((recount.deallocated, recount.died), (0, None));
        assert!(table.is_referenced_more_than_once(&ExtentRange::new(extent, 0..2 * BS)));
        table.check();

        let all = ExtentRange::new(extent, 0..4 * BS);
        let recount = table.recount(&all, -1, Applied::Live(30)).expect("recount failed");
        assert_eq!(
            recount.records.0,
            [
                counted(extent, 0..BS, 0),
                counted(extent, BS..2 * BS, 1),
                counted(extent, 2 * BS..4 * BS, 0)
            ]
        );
        assert_eq!((recount.deallocated, recount.died), (3 * BS, None));
        assert!(!table.is_referenced_more_than_once(&all));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, BS)]);
        table.check();
    }

    /// A shared extent emptied lives on while pins hold it: staging, and allocations of
    /// transactions not yet committed. It dies with the last, and leaves the tree then, to be
    /// freed as it would have been when it emptied.
    #[fuchsia::test]
    async fn pins_hold_an_emptied_shared_extent() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let (first, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        let staging = first.extent;
        // Pending, it isn't alive, but may reach as far as staging does.
        assert!(!table.is_alive(staging));
        assert_eq!(table.extent_size(staging, &device), Some(STAGING_SIZE));
        table.check();
        commit(&mut table, &first, Applied::Live(10));
        let (second, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        assert_eq!(second, ExtentRange::new(staging, BS..2 * BS));

        let recount = table.recount(&first, -1, Applied::Live(20)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(staging, 0..BS, 0)]);
        assert_eq!((recount.deallocated, recount.died), (BS, None));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(staging, OWNER, 0)]);
        table.check();
        // Dropped, the second allocation leaves staging to hold it.
        assert!(table.drop_allocation(&second, OWNER).0.is_empty());
        assert!(table.is_alive(staging));
        assert!(table.dead_extents().is_empty());
        table.check();

        // Sealed, it is replaced, and dies.
        device.flush().await.expect("flush failed");
        let (spare, records) = table.restage(DEFAULT_BRANCH, &device).expect("not restaged");
        assert_eq!(spare, ExtentId::new(DEFAULT_BRANCH, staging.sequence + 1));
        assert_eq!(records.0, [removed(staging, BS)]);
        assert!(!table.is_alive(staging));
        assert_eq!(table.extent_size(staging, &device), None);
        assert_eq!(table.dead_extents(), [(staging, FreeAfter::Flush(20), BS, OWNER)]);
        table.check();
    }

    /// A staging extent that no allocation committed in dies once nothing pins it, unrecorded, and
    /// taking nothing, to be freed at once, as its data may have been written.
    #[fuchsia::test]
    async fn a_staging_extent_nothing_committed_in_dies_unrecorded() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let (range, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        let staging = range.extent;
        assert!(table.drop_allocation(&range, OWNER).0.is_empty());
        assert!(table.is_pinned(staging));
        assert!(table.dead_extents().is_empty());
        table.check();
        // Open still, it isn't replaced.
        assert!(table.restage(DEFAULT_BRANCH, &device).is_none());

        device.flush().await.expect("flush failed");
        let (_, records) = table.restage(DEFAULT_BRANCH, &device).expect("not restaged");
        assert!(records.0.is_empty());
        assert!(!table.is_pinned(staging));
        assert_eq!(table.dead_extents(), [(staging, FreeAfter::Now, 0, INVALID_OBJECT_ID)]);
        assert_eq!(table.dead_bytes(), 0);
        table.check();
    }

    /// An extent a dropped transaction allocated dies, as far as it was allocated, to be freed at
    /// once; one no allocator mints doesn't.
    #[fuchsia::test]
    fn a_dropped_allocation_kills_its_extent() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        let records = table.drop_allocation(&ExtentRange::new(extent, 0..2 * BS), OWNER);
        assert!(records.0.is_empty());
        let unminted = ExtentRange::new(ExtentId::new(0, 7), 0..BS);
        assert!(table.drop_allocation(&unminted, OWNER).0.is_empty());
        assert_eq!(table.dead_extents(), [(extent, FreeAfter::Now, 2 * BS, OWNER)]);
        assert_eq!(table.dead_bytes(), 2 * BS);
        table.check();
    }

    /// The extents of owners deleted die, the staging extent with them, to be freed once a flush
    /// past the deletion is recorded.
    #[fuchsia::test]
    fn owners_deleted_kill_their_extents_and_the_staging_extent() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let (shared, _) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        commit(&mut table, &shared, Applied::Live(10));
        let unshared = ExtentRange::new(table.mint(DEFAULT_BRANCH, &device), 0..2 * BS);
        commit(&mut table, &unshared, Applied::Live(20));
        let other = ExtentRange::new(table.mint(DEFAULT_BRANCH, &device), 0..BS);
        let _ = table.commit(&other, OTHER_OWNER, Applied::Live(20)).expect("commit failed");

        table.kill_owners(&[OWNER], Applied::Live(30));
        let mut dead = table.dead_extents();
        dead.sort_by_key(|(extent, ..)| *extent);
        assert_eq!(
            dead,
            [
                (shared.extent, FreeAfter::RecordedFlush(30), BS, OWNER),
                (unshared.extent, FreeAfter::RecordedFlush(30), 2 * BS, OWNER)
            ]
        );
        assert!(!table.needs_recorded_flush(30));
        assert!(table.needs_recorded_flush(31));
        assert!(table.holds_dead_of(OWNER) && !table.holds_dead_of(OTHER_OWNER));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(other.extent, OTHER_OWNER, BS)]);
        table.check();

        // The next shared allocation takes a new staging extent.
        let (next, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        let staging = ExtentId::new(DEFAULT_BRANCH, other.extent.sequence + 1);
        assert_eq!(next, ExtentRange::new(staging, 0..BS));
        table.check();
    }

    /// Dead extents are freed in the order they died. Those the device fails to free are dead
    /// again, after those that died meanwhile; those it frees are dying until a flush that began
    /// after their frees is done.
    #[fuchsia::test]
    fn frees_go_in_death_order_and_failures_wait_at_the_back() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let kill = |table: &mut ExtentTable| {
            let extent = table.mint(DEFAULT_BRANCH, &device);
            let _ = table.drop_allocation(&ExtentRange::new(extent, 0..BS), OWNER);
            extent
        };
        let [a, b, c] = [(); 3].map(|()| kill(&mut table));
        let freeing = table.start_freeing(0, false);
        assert_eq!(freeing.extents(), [a, b, c]);
        assert_eq!(table.dead().len(), 0);
        let d = kill(&mut table);
        let results = vec![Ok(()), Err(anyhow!("freeing fails")), Ok(())];
        table.finish_freeing(freeing, results).expect_err("all were freed");
        assert_eq!(table.dead().collect::<Vec<_>>(), [d, b]);
        assert_eq!(table.dying(), [a, b, c, d]);
        assert_eq!(table.dead_bytes(), 2 * BS);
        table.check();

        table.seal_begins();
        let freeing = table.start_freeing(0, false);
        assert_eq!(freeing.extents(), [d, b]);
        table.finish_freeing(freeing, vec![Ok(()), Ok(())]).expect("free failed");
        assert_eq!(table.dying(), [a, b, c, d]);
        table.sealed();
        assert_eq!(table.dying(), [b, d]);
        table.seal_begins();
        table.sealed();
        assert_eq!(table.dying(), []);
        table.check();
    }

    /// A transition that fails changes nothing: neither the extent, nor the bytes counted.
    #[fuchsia::test]
    fn a_failed_transition_changes_nothing() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        commit(&mut table, &ExtentRange::new(extent, 0..2 * BS), Applied::Live(10));
        let second = ExtentRange::new(extent, BS..2 * BS);
        let _ = table.recount(&second, 1, Applied::Live(20)).expect("recount failed");

        let inconsistent = |error: anyhow::Error| {
            assert!(FxfsError::Inconsistent.matches(&error), "{error:?}");
        };
        let more = ExtentRange::new(extent, 2 * BS..3 * BS);
        inconsistent(table.commit(&more, OTHER_OWNER, Applied::Live(30)).expect_err("committed"));
        let empty = ExtentRange::new(extent, 2 * BS..2 * BS);
        inconsistent(table.commit(&empty, OWNER, Applied::Live(30)).expect_err("committed"));
        // Its first part could be deallocated, and its second lose a reference, but not its third.
        let past = ExtentRange::new(extent, 0..3 * BS);
        inconsistent(table.recount(&past, -1, Applied::Live(30)).expect_err("recounted"));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, 2 * BS)]);
        assert!(table.is_referenced_more_than_once(&second));
        assert_eq!(table.alive_bytes(), 2 * BS);

        // Nor can what isn't allocated be referenced: a pending extent, or one the table doesn't
        // hold.
        let (pending, _) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        inconsistent(table.recount(&pending, 1, Applied::Live(40)).expect_err("recounted"));
        let unknown = ExtentRange::new(ExtentId::new(DEFAULT_BRANCH, 1000), 0..BS);
        inconsistent(table.recount(&unknown, -1, Applied::Live(40)).expect_err("recounted"));
        assert!(table.dead_extents().is_empty());
        table.check();
    }

    /// A mount finds the extents alive in the tree, with their sizes and references; those of
    /// owners deleted, those of which nothing is allocated, and those dying when the tree was
    /// compacted are dead. Orphans die after replay, and minting resumes past them.
    #[fuchsia::test]
    fn a_mount_loads_the_extents_alive_and_kills_the_rest() {
        let device = FakeDevice::new(64, BS as u32);
        let [alive, empty, deleted, gone] = [1, 2, 3, 4].map(|s| ExtentId::new(DEFAULT_BRANCH, s));
        let mut loading = Loading::default();
        for (extent, range, count, owner) in [
            (alive, 0..BS, 1, OWNER),
            (alive, BS..2 * BS, 2, OWNER),
            (alive, 2 * BS..3 * BS, 0, OWNER),
            (empty, 0..BS, 0, OWNER),
            (deleted, 0..BS, 1, OTHER_OWNER),
        ] {
            loading.add(&AllocatorKey::new(extent, range), count, owner).expect("add failed");
        }
        let wrong = loading.add(&AllocatorKey::new(alive, 3 * BS..4 * BS), 1, OTHER_OWNER);
        assert!(FxfsError::Inconsistent.matches(&wrong.expect_err("added")));

        let mut table = ExtentTable::default();
        table.load(loading, vec![gone, alive], &HashSet::from([OTHER_OWNER]));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(alive, OWNER, 2 * BS)]);
        assert_eq!(table.extent_size(alive, &device), Some(3 * BS));
        assert!(table.is_referenced_more_than_once(&ExtentRange::new(alive, BS..2 * BS)));
        let mut dead = table.dead_extents();
        assert_eq!(dead.remove(0), (gone, FreeAfter::Now, 0, INVALID_OBJECT_ID));
        dead.sort_by_key(|(extent, ..)| *extent);
        assert_eq!(
            dead,
            [
                (empty, FreeAfter::Now, BS, OWNER),
                (deleted, FreeAfter::RecordedFlush(0), BS, OTHER_OWNER)
            ]
        );
        table.check();

        let orphan = ExtentId::new(DEFAULT_BRANCH, 0);
        assert_eq!(table.kill_orphans(DEFAULT_BRANCH, 0..2), 1);
        let orphaned = (orphan, FreeAfter::RecordedFlush(0), 0, INVALID_OBJECT_ID);
        assert_eq!(table.dead_extents().last(), Some(&orphaned));
        table.mint_from(5);
        assert_eq!(table.mint(DEFAULT_BRANCH, &device), ExtentId::new(DEFAULT_BRANCH, 5));
        table.check();
    }

    /// Replay, which pins aren't in, kills an extent with its last deallocation, to be freed at
    /// once; an allocation replayed in it after revives it, as far as that allocation reaches, as
    /// one of a shared extent that pins kept alive, live.
    #[fuchsia::test]
    fn replay_kills_an_emptied_extent_at_once_and_revives_it() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = ExtentId::new(DEFAULT_BRANCH, 5);
        let second = ExtentRange::new(extent, BS..2 * BS);
        commit(&mut table, &second, Applied::Replayed);
        let recount = table.recount(&second, -1, Applied::Replayed).expect("recount failed");
        assert_eq!(recount.records.0, [removed(extent, 2 * BS)]);
        assert_eq!(table.dead_extents(), [(extent, FreeAfter::Now, 2 * BS, OWNER)]);
        table.check();

        commit(&mut table, &ExtentRange::new(extent, 0..BS), Applied::Replayed);
        assert!(table.dead_extents().is_empty());
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, BS)]);
        assert_eq!(table.extent_size(extent, &device), Some(BS));
        assert_eq!((table.alive_bytes(), table.dead_bytes()), (BS, 0));
    }
}
