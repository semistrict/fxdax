// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax's own: the lives of the allocator's extents, from their minting to the seal of their
//! frees (backlog doc-4; the allocator's "Dead extents", "Shared extents" and "Orphaned extents").
//!
//! [`ExtentTable`] knows each extent the allocator minted or found in its tree, in one of these
//! states, each with the transitions that take an extent into it and out of it:
//!
//! - Minted for an allocation that isn't shared, whose transaction hasn't committed (`mint`): the
//!   table doesn't hold it yet. `commit` makes it allocated. `drop_allocation` kills it, as far as
//!   it was allocated, as its data may have been written.
//! - Pending ([`ExtentState::Pending`]), minted by `stage` for shared allocations, none of which
//!   has committed: the staging extent, or one transactions not yet committed have allocated in.
//!   Each of those pins it, as being the staging extent does. `commit` makes it allocated. It dies
//!   with its last pin, as nothing records it: as `drop_allocation` drops it, or as `stage`
//!   replaces it as the staging extent. It takes as far as the allocations dropped in it reached,
//!   as an allocated one does (see below).
//! - Allocated ([`ExtentState::Allocated`]), by `commit` or `load`: alive, with some of it
//!   allocated. It reaches as far as its last range, allocated, deallocated or dropped (a shared
//!   allocation `drop_allocation` drops leaves its range taken, as an extent is written once, and
//!   the next allocation in it starts past it: TASK-211), and each range allocated is
//!   referenced by one file's extents, or by more (reflinks). `commit` allocates more of it, and
//!   `recount` changes the references to it, which leaves it allocated while some of it is. Once
//!   `recount` deallocates the last of it, it is empty if pins hold it, as a shared one's still
//!   may, and dies otherwise. `kill_owners` kills it once its owner is deleted, whatever holds it.
//! - Empty ([`ExtentState::Empty`]), by `recount`: alive, shared, with nothing of it allocated, as
//!   pins hold it. `recount` refuses it, as nothing of it can be referenced. `commit` makes it
//!   allocated again, as large as it was. It dies with its last pin, as a pending one does, to be
//!   freed as it would have been when it emptied; and `kill_owners` kills it with its owner.
//! - Dead, by any of those, or by a mount: `load` kills the extents of owners deleted, those of
//!   which nothing is allocated, and those dying when the tree was last compacted; and after
//!   replay, `kill_orphans` kills those a crash left. It is queued, in the order extents died,
//!   until its [`FreeAfter`] allows its free, when `start_freeing` takes it to be freed. In
//!   replay, `commit` revives it (see below).
//! - Being freed, by `start_freeing`: out of the queue, in the batch the device frees, dying still,
//!   as the device may never make its free. `finish_freeing` makes it freed, or, if the device
//!   didn't free it, dead again, at the back of the queue.
//! - Freed, by `finish_freeing`, whose free a crash might undo until a device flush that began
//!   after it is done: the allocator's info holds it as dying meanwhile. Once such a flush begins
//!   (`seal_begins`) and is done (`sealed`), the table forgets it.
//!
//! Each transition checks first, and changes nothing if a check fails. Each keeps two totals: the
//! bytes the extents alive take, whole, and those the dead take, with those being freed, which the
//! device holds until they are.
//!
//! Each also returns the records it implies for the allocator's tree ([`Records`]), which the
//! allocator merges into it:
//!
//! - `commit`: the allocation's range, counted once.
//! - `recount`: the parts of the range whose references change, with their new counts. A part none
//!   references any more is counted 0, so that its extent's size is known while the rest is alive.
//! - The removal from the tree of an extent that dies, in three cases. One is `recount`'s, with
//!   the extent's last deallocation, which its transaction journals. Another is an empty extent's,
//!   with its last pin (`stage`, `drop_allocation`), which no transaction journals:
//!   replay, which pins aren't in, kills the extent with its last deallocation instead, unless
//!   the tree was compacted since. The third is `load`'s, of an extent it finds with nothing
//!   allocated: one empty when the tree was last compacted, as the staging extent emptied may be,
//!   whose filesystem stopped while pins still held it, so that the removal its last pin's going
//!   writes never came. Its records, all counted 0, stayed in the tree for good otherwise, as no
//!   compaction filters them out, and fsck reported the extent as an `ExtentMismatch`
//!   (TASK-119.22). The tree's next compaction writes the removal.
//!
//! The other deaths write nothing. Those of owners deleted (`kill_owners`, and `load`'s) leave
//! their records to a compaction of all the tree's layers, which filters them out once they are
//! freed (the allocator's "Compactions"). The tree has no records of pending extents, of those of
//! transactions that dropped, of orphans, nor of those dying when it was last compacted, but for
//! removals that hide their records in older layers.
//!
//! Replay applies the transactions the journal holds, which pins aren't in: there, an extent dies
//! with its last deallocation. An allocation replayed in an extent that died in this replay, and
//! isn't yet freed, revives it, as one of a shared extent that pins kept alive, live. One that replay
//! discards, as it does a transaction whose writes a crash lost, doesn't; and the next replay reads
//! it again, unless the journal recorded the discard: what a mount finds dead, however it died, in
//! replay or as it loads the tree, if what replay discarded names it, is freed only after that
//! record, with a flush's (`FreeAfter::Settling`, `ExtentTable::settle_replay`). Two bugs were a
//! death a mount found that this missed (CI runs 41876989384 and 42196106048, TASK-210).

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
    /// The dead extents being freed (`start_freeing`), in the order they died, until their frees'
    /// results come back (`finish_freeing`): dying, as a crash may come first.
    freeing: Vec<DeadExtent>,
    /// The bytes the dead extents take, with those being freed.
    dead_bytes: u64,
    /// The extents freed since the last device flush began, whose frees a crash might undo.
    unsealed: Vec<ExtentId>,
    /// The extents freed before the device flush in progress began, whose frees it seals.
    sealing: Vec<ExtentId>,
    /// The extents minted, of which no transaction has committed an allocation, and none has died:
    /// a crash that loses their transactions leaves them on the device, nothing knowing of them,
    /// and a mount can't find them by their sequence numbers if a compaction recorded later ones
    /// meanwhile (`recorded_dying`).
    uncommitted: HashSet<ExtentId>,
    /// The sequence number of the next extent minted.
    next_sequence: u64,
}

/// The state of an extent the table holds, which hasn't died.
#[derive(Debug)]
enum ExtentState {
    /// Shared, with no allocation in it committed yet. `pins` hold it: one while it is the staging
    /// extent, and one for each allocation in it of a transaction not yet committed. It takes
    /// `size` bytes: as far as allocations dropped in it reached.
    Pending { pins: NonZero<usize>, size: u64 },
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

    /// The bytes the extent takes: as far as its last range, allocated or not, or dropped.
    fn size(&self) -> u64 {
        match self {
            ExtentState::Allocated { info, .. } => info.size,
            ExtentState::Pending { size, .. } | ExtentState::Empty { size, .. } => *size,
        }
    }

    /// Has the extent take as far as `end`, if it doesn't: returns how many bytes more it takes.
    fn reach(&mut self, end: u64) -> u64 {
        let size = match self {
            ExtentState::Allocated { info, .. } => &mut info.size,
            ExtentState::Pending { size, .. } | ExtentState::Empty { size, .. } => size,
        };
        let more = end.saturating_sub(*size);
        *size += more;
        more
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
            ExtentState::Pending { pins, .. } | ExtentState::Empty { pins, .. } => pins.get(),
            ExtentState::Allocated { pins, .. } => *pins,
        }
    }

    /// Adds a pin.
    fn pin(&mut self) {
        match self {
            ExtentState::Pending { pins, .. } | ExtentState::Empty { pins, .. } => {
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
    /// Nothing: it died in a transaction dropped, which wrote no record of it, or before the
    /// tree was last compacted, unless what replay discarded names it (`settle_replay`): one
    /// minted then, its allocation not yet committed, a transaction since can allocate in.
    Now,
    /// The journal up to this offset, where it died, so that a crash can't bring it back. A replay
    /// that verifies the checksums of writes to it also replays its deallocation, which spares
    /// them.
    Flush(u64),
    /// That, and a record of the flush (see `Journal::flush_device`): its owner was deleted, a
    /// crash left it (an orphan), or a mount found it dead, and its replay discarded transactions
    /// that name it (`settle_replay`), so that a replay may verify the checksums of writes to it,
    /// unless the record says they are on the device, or the journal before it, the discard.
    /// Offset 0 is any flush.
    RecordedFlush(u64),
    /// A mount found it dead, in replay or empty in the tree, and hasn't settled when it may be
    /// freed (`ExtentTable::settle_replay`), which nothing allows until then. A death replay finds
    /// isn't final while what replay discarded may be read again: a shared extent emptied lives on
    /// with its pins, which aren't journaled, and takes allocations again, which replay discards
    /// as it does a transaction whose writes a crash lost. A crash before the journal records the
    /// discard has the next replay read the allocation again and verify its write, which it can't
    /// in an extent freed (CI run 41876989384). If what replay discarded names it, then, it waits
    /// for a recorded flush, which the discard's record comes before.
    Settling,
}

impl FreeAfter {
    /// Whether the extent can be freed now that the journal is flushed to `log_file_offset`, with
    /// a record of it if `recorded`.
    fn allows(&self, log_file_offset: u64, recorded: bool) -> bool {
        match *self {
            FreeAfter::Now => true,
            FreeAfter::Flush(offset) => offset < log_file_offset,
            FreeAfter::RecordedFlush(offset) => recorded && offset < log_file_offset,
            FreeAfter::Settling => false,
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
    /// past the transaction, with a record of the flush if `recorded`; and in replay, as the mount
    /// settles it (`FreeAfter::Settling`), with a record of any flush if `recorded`.
    fn free_after(self, recorded: bool) -> FreeAfter {
        match (self, recorded) {
            (Applied::Live(offset), false) => FreeAfter::Flush(offset),
            (Applied::Live(offset), true) => FreeAfter::RecordedFlush(offset),
            (Applied::Replayed, false) => FreeAfter::Settling,
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

/// What dropping an allocation did (`ExtentTable::drop_allocation`).
#[must_use]
#[derive(Debug)]
pub(super) struct Dropped {
    pub(super) records: Records,
    /// The bytes of the range its extent takes from now on, which it didn't: those of a shared
    /// extent that lives on. Those are as allocated, until the extent dies; the rest are free, at
    /// once or once the extent is freed.
    pub(super) kept: u64,
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

impl ExtentTable {
    /// Mints an extent of `branch`, opened on `device`: open until a seal, as none can have sealed
    /// an extent not yet minted.
    pub(super) fn mint(&mut self, branch: u64, device: &dyn Device) -> ExtentId {
        let extent = ExtentId::new(branch, self.next_sequence);
        self.next_sequence += 1;
        self.uncommitted.insert(extent);
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
            self.replace_staging(branch, owner_object_id, device)
        };
        let staging = self.staging.as_mut().unwrap();
        let range = ExtentRange::new(staging.extent, staging.next..staging.next + len);
        staging.next += len;
        self.extents.get_mut(&range.extent).expect("the staging extent is held").pin();
        (range, records)
    }

    /// Makes a new extent of `branch`, opened on `device`, the staging extent of
    /// `owner_object_id`, in place of the last, which it no longer pins: the records that implies.
    fn replace_staging(
        &mut self,
        branch: u64,
        owner_object_id: u64,
        device: &dyn Device,
    ) -> Records {
        let records = match self.staging.take() {
            Some(old) => self.unpin(old.extent),
            None => Records::default(),
        };
        let extent = self.mint(branch, device);
        self.extents.insert(extent, ExtentState::Pending { pins: NonZero::<usize>::MIN, size: 0 });
        self.staging = Some(Staging { extent, owner_object_id, next: 0 });
        records
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
        self.uncommitted.remove(&extent);
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
            Some(ExtentState::Pending { pins, size }) => (new(size), pins.get()),
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
    /// they go, and it takes the range meanwhile, as it does a range allocated: an extent is written
    /// once, and its next allocation starts past it (TASK-211).
    pub(super) fn drop_allocation(&mut self, range: &ExtentRange, owner_object_id: u64) -> Dropped {
        if self.is_pinned(range.extent) {
            let state = self.extents.get_mut(&range.extent).expect("a pinned extent is held");
            let before = state.size();
            self.alive_bytes += state.reach(range.range.end);
            let records = self.unpin(range.extent);
            // Dead with its last pin, it takes nothing alive: what it took is freed with it.
            let kept = match self.extents.contains_key(&range.extent) {
                true => range.range.end.saturating_sub(before.max(range.range.start)),
                false => 0,
            };
            return Dropped { records, kept };
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
        Dropped { records: Records::default(), kept: 0 }
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
            ExtentState::Pending { pins, .. } | ExtentState::Empty { pins, .. }
                if pins.get() > 1 =>
            {
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
    /// Returns the removals of those of which nothing is allocated, whose records the tree holds.
    pub(super) fn load(
        &mut self,
        loading: Loading,
        dying: Vec<ExtentId>,
        marked_for_deletion: &HashSet<u64>,
    ) -> Records {
        let mut records = Records::default();
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
                // Empty when the tree was compacted, as the staging extent emptied may be: an
                // allocation in it since is in the journal, which replay may discard.
                let free_after = FreeAfter::Settling;
                self.queue_dead(DeadExtent { free_after, extent, size, owner_object_id });
                records.add_removal(extent, size);
            }
        }
        records
    }

    /// Kills the extents of `branch` with `sequences`, and those `discarded` names, of what replay
    /// read and discarded, wherever they are, that aren't alive: those a crash left, whose
    /// transactions were lost. Those `discarded` names, which a replay after may read again and
    /// verify, are freed with the next flush of the device with a record of it; the rest at once,
    /// as nothing a replay reads names them: the mount frees them and seals their frees before it
    /// writes past its floor (`Allocator::on_replay_complete`). Of another writer's range than
    /// `sequences`, those `discarded` names were found by no window (TASK-149.41). Returns how
    /// many.
    pub(super) fn kill_orphans(
        &mut self,
        branch: u64,
        sequences: Range<u64>,
        discarded: &HashSet<ExtentId>,
    ) -> usize {
        let dying: HashSet<_> = self.dying().into_iter().collect();
        let named = (discarded.iter().copied())
            .filter(|extent| extent.branch == branch && !sequences.contains(&extent.sequence));
        let orphans: Vec<_> = (sequences.clone())
            .map(|sequence| ExtentId::new(branch, sequence))
            .chain(named)
            .filter(|extent| !self.is_alive(*extent) && !dying.contains(extent))
            .collect();
        for &extent in &orphans {
            let free_after = match discarded.contains(&extent) {
                true => FreeAfter::RecordedFlush(0),
                false => FreeAfter::Now,
            };
            self.queue_dead(DeadExtent {
                free_after,
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
        self.uncommitted.remove(&dead.extent);
        self.dead_bytes += dead.size;
        self.dead.push_back(dead);
    }

    /// Settles when the extents a mount found dead may be freed, once replay is done: those that
    /// `discarded`, what replay discarded of the journal, names, however they died, once a flush
    /// is recorded, which the journal's record of the discard comes before, as a replay after
    /// reads it again until then and verifies its writes: as in a shared extent emptied (CI run
    /// 41876989384), or in one minted with no allocation committed as the tree was last
    /// compacted, which `load` finds dying (TASK-210). The rest as they died, those `Settling` at
    /// once, as a replay after reads what it says of them as this one did: a full device's mount
    /// needs their room to renew the journal's tail, before the flush that records the discard.
    pub(super) fn settle_replay(&mut self, discarded: &HashSet<ExtentId>) {
        for dead in &mut self.dead {
            dead.free_after = match (discarded.contains(&dead.extent), dead.free_after) {
                (true, FreeAfter::Flush(offset) | FreeAfter::RecordedFlush(offset)) => {
                    FreeAfter::RecordedFlush(offset)
                }
                (true, FreeAfter::Now | FreeAfter::Settling) => FreeAfter::RecordedFlush(0),
                (false, FreeAfter::Settling) => FreeAfter::Now,
                (false, free_after) => free_after,
            };
        }
    }

    /// Starts freeing the dead extents that a flush of the journal to `log_file_offset`, recorded
    /// if `recorded`, allows to be freed, and returns them, in the order they died, to be freed.
    /// One batch is freed at a time.
    #[must_use]
    pub(super) fn start_freeing(&mut self, log_file_offset: u64, recorded: bool) -> Vec<ExtentId> {
        debug_assert!(self.freeing.is_empty(), "frees overlap");
        let (free, wait): (VecDeque<_>, VecDeque<_>) = std::mem::take(&mut self.dead)
            .into_iter()
            .partition(|dead| dead.free_after.allows(log_file_offset, recorded));
        self.dead = wait;
        self.freeing = free.into();
        self.freeing.iter().map(|dead| dead.extent).collect()
    }

    /// fxdax: the extents being freed, each with how it died, said for a log (TASK-121).
    pub(super) fn freeing_described(&self) -> Vec<(ExtentId, String)> {
        self.freeing.iter().map(|dead| (dead.extent, format!("{dead:?}"))).collect()
    }

    /// Finishes freeing the extents `start_freeing` returned, with `results`, those of their
    /// frees, in order. The extents freed are unsealed until a device flush seals their frees;
    /// those not freed are dead again, at the back of the queue, and tried again later.
    pub(super) fn finish_freeing(&mut self, results: Vec<Result<(), Error>>) -> Result<(), Error> {
        let mut first_error = None;
        for (dead, result) in std::mem::take(&mut self.freeing).into_iter().zip(results) {
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

    /// The lowest sequence number of `branch` of the extents alive.
    pub(super) fn lowest_sequence(&self, branch: u64) -> Option<u64> {
        let extents = self.extents.keys().filter(|extent| extent.branch == branch);
        extents.map(|extent| extent.sequence).min()
    }

    /// What a compaction records as `dying`, of which the tree it leaves has no records: those
    /// `dying`, and those minted with no allocation committed yet, which replay revives if theirs
    /// is in the journal, and a crash leaves to be freed if not.
    pub(super) fn recorded_dying(&self) -> Vec<ExtentId> {
        let mut dying = self.dying();
        dying.extend(self.uncommitted.iter().copied());
        dying.sort();
        dying.dedup();
        dying
    }

    /// The extents dead, those being freed, and those freed whose frees a crash might undo.
    pub(super) fn dying(&self) -> Vec<ExtentId> {
        let mut dying: Vec<_> = self
            .dead()
            .chain(self.freeing.iter().map(|dead| dead.extent))
            .chain(self.unsealed.iter().copied())
            .chain(self.sealing.iter().copied())
            .collect();
        dying.sort();
        dying.dedup();
        dying
    }

    /// Whether the table holds an extent of any of `branches`.
    pub(super) fn holds_any_of(&self, branches: &HashSet<u64>) -> bool {
        self.extents.keys().any(|extent| branches.contains(&extent.branch))
    }

    /// The owner `extent`'s allocations are recorded under, if it is alive.
    pub(super) fn owner(&self, extent: ExtentId) -> Option<u64> {
        self.extents.get(&extent)?.owner()
    }

    /// Has the extent of `range` reach as far as its end, if it is alive: an allocation of it that
    /// dropped took it, which the extent keeps, as it is written once (`drop_allocation`), and
    /// `AllocatorMutation::Reach` records. Returns the range's record, counted 0, which sizes the
    /// extent as a mount loads it. One not alive takes nothing of it: dead, it goes whole.
    pub(super) fn reach(&mut self, range: &ExtentRange) -> Records {
        let mut records = Records::default();
        let Some(state) = self.extents.get_mut(&range.extent).filter(|state| state.is_alive())
        else {
            return records;
        };
        let owner_object_id = state.owner().expect("an extent alive has an owner");
        self.alive_bytes += state.reach(range.range.end);
        records.add_count(range.extent, range.range.clone(), 0, owner_object_id);
        records
    }

    /// The bytes the table counts `extent` taking, in whatever state, dead and not yet freed too:
    /// the staging extent, while it is open, as far as it may yet reach.
    pub(super) fn counted(&self, extent: ExtentId, device: &dyn Device) -> u64 {
        let open = self.staging.as_ref().is_some_and(|staging| staging.extent == extent)
            && device.is_open(extent);
        if open {
            return STAGING_SIZE;
        }
        let dead = self.dead.iter().chain(&self.freeing).filter(|dead| dead.extent == extent);
        let dead = dead.map(|dead| dead.size).max().unwrap_or(0);
        self.extents.get(&extent).map_or(0, ExtentState::size).max(dead)
    }

    /// Whether `extent` is alive.
    pub(super) fn is_alive(&self, extent: ExtentId) -> bool {
        self.extents.get(&extent).is_some_and(ExtentState::is_alive)
    }

    /// Whether pins hold `extent`: the staging extent, or one transactions not yet committed have
    /// allocated in.
    fn is_pinned(&self, extent: ExtentId) -> bool {
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

    /// Whether an extent of `owner_object_id`'s is dead, or being freed.
    pub(super) fn holds_dead_of(&self, owner_object_id: u64) -> bool {
        self.dead.iter().chain(&self.freeing).any(|dead| dead.owner_object_id == owner_object_id)
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
        /// Checks the table's invariants: its byte totals are its extents', each extent's state is
        /// one it can be in, and the staging extent is pinned. Not that no extent alive is dead
        /// too: one is, if an allocation in it commits after its owner's extents were killed.
        fn check(&self) {
            let alive_bytes: u64 = self.extents.values().map(ExtentState::size).sum();
            assert_eq!(self.alive_bytes, alive_bytes, "the bytes alive");
            let dead_bytes: u64 = self.dead.iter().chain(&self.freeing).map(|dead| dead.size).sum();
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
        assert!(early.is_empty());
        table.finish_freeing(vec![]).expect("free failed");
        let freeing = table.start_freeing(31, false);
        assert_eq!(freeing, [extent]);
        // It is counted until it is freed.
        assert_eq!(table.dead_bytes(), 2 * BS);
        table.check();
        table.finish_freeing(vec![Ok(())]).expect("free failed");
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
    /// transactions not yet committed. One of those committing allocates it again, as large as it
    /// was, with none of the references it had. It dies with the last pin, and leaves the tree
    /// then, to be freed as it would have been when it last emptied.
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
        let (second, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        assert_eq!(second, ExtentRange::new(staging, BS..2 * BS));
        let (third, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        assert_eq!(third, ExtentRange::new(staging, 2 * BS..3 * BS));
        commit(&mut table, &second, Applied::Live(10));
        // A reflink of it.
        let recount = table.recount(&second, 1, Applied::Live(15)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(staging, BS..2 * BS, 2)]);

        // Both its references gone at once, it is empty.
        let recount = table.recount(&second, -2, Applied::Live(20)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(staging, BS..2 * BS, 0)]);
        assert_eq!((recount.deallocated, recount.died), (BS, None));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(staging, OWNER, 0)]);
        assert_eq!(table.alive_bytes(), 2 * BS);
        table.check();

        // Committed, the first allocation allocates it again, its size and the bytes it takes as
        // they were.
        commit(&mut table, &first, Applied::Live(30));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(staging, OWNER, BS)]);
        assert_eq!(table.alive_bytes(), 2 * BS);
        assert!(!table.is_referenced_more_than_once(&ExtentRange::new(staging, 0..2 * BS)));
        let recount = table.recount(&first, -1, Applied::Live(40)).expect("recount failed");
        assert_eq!(recount.records.0, [counted(staging, 0..BS, 0)]);
        assert_eq!((recount.deallocated, recount.died), (BS, None));
        assert_eq!(table.alive().collect::<Vec<_>>(), [(staging, OWNER, 0)]);
        table.check();

        // Dropped, the third allocation leaves staging to hold it, which takes its range.
        let dropped = table.drop_allocation(&third, OWNER);
        assert!(dropped.records.0.is_empty());
        assert_eq!((dropped.kept, table.alive_bytes()), (BS, 3 * BS));
        assert!(table.is_alive(staging));
        assert!(table.dead_extents().is_empty());
        table.check();

        // Sealed, it is replaced by the next shared allocation's, and dies.
        device.flush().await.expect("flush failed");
        let (next, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        let spare = ExtentId::new(DEFAULT_BRANCH, staging.sequence + 1);
        assert_eq!(next, ExtentRange::new(spare, 0..BS));
        assert_eq!(records.0, [removed(staging, 3 * BS)]);
        assert!(!table.is_alive(staging));
        assert_eq!(table.extent_size(staging, &device), None);
        assert_eq!(table.dead_extents(), [(staging, FreeAfter::Flush(40), 3 * BS, OWNER)]);
        table.check();
    }

    /// A staging extent that no allocation committed in dies once nothing pins it, unrecorded, and
    /// taking what the allocations dropped in it reached, to be freed at once, as its data may have
    /// been written.
    #[fuchsia::test]
    async fn a_staging_extent_nothing_committed_in_dies_unrecorded() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let (range, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert!(records.0.is_empty());
        let staging = range.extent;
        let dropped = table.drop_allocation(&range, OWNER);
        assert!(dropped.records.0.is_empty());
        assert_eq!(dropped.kept, BS);
        assert!(table.is_pinned(staging));
        assert!(table.dead_extents().is_empty());
        assert_eq!(table.alive_bytes(), BS);
        table.check();

        // Sealed, it is replaced by the next shared allocation's.
        device.flush().await.expect("flush failed");
        let (next, records) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        assert_ne!(next.extent, staging);
        assert!(records.0.is_empty());
        assert!(!table.is_pinned(staging));
        assert_eq!(table.dead_extents(), [(staging, FreeAfter::Now, BS, INVALID_OBJECT_ID)]);
        assert_eq!((table.alive_bytes(), table.dead_bytes()), (0, BS));
        table.check();
    }

    /// An allocation dropped in a shared extent that lives on leaves its range taken, as the extent
    /// is written once and its next allocation starts past it: the extent takes it from then on,
    /// once, whichever of its allocations commits or drops first. Given back, the range was taken
    /// twice: a full device's filesystem held more than the device, and its next mount found too
    /// little free to reserve its metadata (TASK-211).
    #[fuchsia::test]
    async fn a_shared_extent_takes_what_was_dropped_in_it_once() {
        let device = FakeDevice::new(64, BS as u32);
        for drop_first in [true, false] {
            let mut table = ExtentTable::default();
            let (first, _) = table.stage(DEFAULT_BRANCH, OWNER, 2 * BS, &device);
            let (second, _) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
            assert_eq!(second, ExtentRange::new(first.extent, 2 * BS..3 * BS));
            let kept = if drop_first {
                let dropped = table.drop_allocation(&first, OWNER);
                assert!(dropped.records.0.is_empty());
                assert_eq!(table.alive_bytes(), 2 * BS);
                commit(&mut table, &second, Applied::Live(10));
                dropped.kept
            } else {
                commit(&mut table, &second, Applied::Live(10));
                // The second's commit took the extent as far as its end, the first's range too.
                assert_eq!(table.alive_bytes(), 3 * BS);
                table.drop_allocation(&first, OWNER).kept
            };
            assert_eq!(kept, if drop_first { 2 * BS } else { 0 });
            assert_eq!(table.alive().collect::<Vec<_>>(), [(first.extent, OWNER, BS)]);
            assert_eq!(table.extent_size(first.extent, &device), Some(STAGING_SIZE));
            assert_eq!(table.alive_bytes(), 3 * BS);
            table.check();
        }
    }

    /// An extent a dropped transaction allocated dies, as far as it was allocated, to be freed at
    /// once; one no allocator mints doesn't.
    #[fuchsia::test]
    fn a_dropped_allocation_kills_its_extent() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        let dropped = table.drop_allocation(&ExtentRange::new(extent, 0..2 * BS), OWNER);
        assert!(dropped.records.0.is_empty());
        assert_eq!(dropped.kept, 0);
        let unminted = ExtentRange::new(ExtentId::new(0, 7), 0..BS);
        assert!(table.drop_allocation(&unminted, OWNER).records.0.is_empty());
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
        assert_eq!(freeing, [a, b, c]);
        assert_eq!(table.dead().len(), 0);
        let d = kill(&mut table);
        let results = vec![Ok(()), Err(anyhow!("freeing fails")), Ok(())];
        table.finish_freeing(results).expect_err("all were freed");
        assert_eq!(table.dead().collect::<Vec<_>>(), [d, b]);
        assert_eq!(table.dying(), [a, b, c, d]);
        assert_eq!(table.dead_bytes(), 2 * BS);
        table.check();

        table.seal_begins();
        let freeing = table.start_freeing(0, false);
        assert_eq!(freeing, [d, b]);
        table.finish_freeing(vec![Ok(()), Ok(())]).expect("free failed");
        assert_eq!(table.dying(), [a, b, c, d]);
        table.sealed();
        assert_eq!(table.dying(), [b, d]);
        table.seal_begins();
        table.sealed();
        assert_eq!(table.dying(), []);
        table.check();
    }

    /// Extents being freed are dying, as the device may never make their frees, and their owners
    /// still hold dead extents, until their frees are done (TASK-79.22).
    #[fuchsia::test]
    fn extents_being_freed_are_dying() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = table.mint(DEFAULT_BRANCH, &device);
        commit(&mut table, &ExtentRange::new(extent, 0..BS), Applied::Live(10));
        table.kill_owners(&[OWNER], Applied::Live(20));

        let freeing = table.start_freeing(21, true);
        assert_eq!(freeing, [extent]);
        assert_eq!(table.dead().len(), 0);
        assert_eq!(table.dying(), [extent]);
        assert!(table.holds_dead_of(OWNER));
        table.check();
        table.finish_freeing(vec![Ok(())]).expect("free failed");
        assert_eq!(table.dying(), [extent]);
        assert!(!table.holds_dead_of(OWNER));
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

        // Nor can what isn't allocated be referenced, nor deallocated: a pending extent, an empty
        // one, or one the table doesn't hold.
        let (shared, _) = table.stage(DEFAULT_BRANCH, OWNER, BS, &device);
        inconsistent(table.recount(&shared, 1, Applied::Live(40)).expect_err("recounted"));
        // Committed, then deallocated, it is empty, as staging holds it.
        commit(&mut table, &shared, Applied::Live(40));
        let _ = table.recount(&shared, -1, Applied::Live(50)).expect("recount failed");
        inconsistent(table.recount(&shared, 1, Applied::Live(60)).expect_err("recounted"));
        inconsistent(table.recount(&shared, -1, Applied::Live(60)).expect_err("recounted"));
        let unknown = ExtentRange::new(ExtentId::new(DEFAULT_BRANCH, 1000), 0..BS);
        inconsistent(table.recount(&unknown, -1, Applied::Live(60)).expect_err("recounted"));
        let mut alive: Vec<_> = table.alive().collect();
        alive.sort();
        assert_eq!(alive, [(extent, OWNER, 2 * BS), (shared.extent, OWNER, 0)]);
        assert_eq!(table.alive_bytes(), 3 * BS);
        assert!(table.dead_extents().is_empty());
        table.check();
    }

    /// A mount finds the extents alive in the tree, with their sizes and references; those of
    /// owners deleted, those of which nothing is allocated, and those dying when the tree was
    /// compacted are dead, and those of which nothing is allocated leave the tree (TASK-119.22).
    /// Orphans die after replay, and minting resumes past them.
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
        let records = table.load(loading, vec![gone, alive], &HashSet::from([OTHER_OWNER]));
        assert_eq!(records.0, [removed(empty, BS)]);
        assert_eq!(table.alive().collect::<Vec<_>>(), [(alive, OWNER, 2 * BS)]);
        assert_eq!(table.extent_size(alive, &device), Some(3 * BS));
        assert!(table.is_referenced_more_than_once(&ExtentRange::new(alive, BS..2 * BS)));
        let mut dead = table.dead_extents();
        assert_eq!(dead.remove(0), (gone, FreeAfter::Now, 0, INVALID_OBJECT_ID));
        dead.sort_by_key(|(extent, ..)| *extent);
        assert_eq!(
            dead,
            [
                (empty, FreeAfter::Settling, BS, OWNER),
                (deleted, FreeAfter::RecordedFlush(0), BS, OTHER_OWNER)
            ]
        );
        table.check();

        let orphan = ExtentId::new(DEFAULT_BRANCH, 0);
        assert_eq!(table.kill_orphans(DEFAULT_BRANCH, 0..2, &HashSet::new()), 1);
        let orphaned = (orphan, FreeAfter::Now, 0, INVALID_OBJECT_ID);
        assert_eq!(table.dead_extents().last(), Some(&orphaned));
        table.mint_from(5);
        assert_eq!(table.mint(DEFAULT_BRANCH, &device), ExtentId::new(DEFAULT_BRANCH, 5));
        table.check();
    }

    /// Orphans die where they are sought, and wherever what replay discarded names them, outside
    /// that too, as another writer's range's are (TASK-149.41): those it names after a recorded
    /// flush, the rest at once. Nothing alive, nor anything dying already, dies again.
    #[fuchsia::test]
    fn orphans_die_where_sought_and_where_what_replay_discarded_names_them() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let alive = table.mint(DEFAULT_BRANCH, &device);
        commit(&mut table, &ExtentRange::new(alive, 0..BS), Applied::Live(0));
        let [sought, far, other_branch] = [
            ExtentId::new(DEFAULT_BRANCH, 20),
            ExtentId::new(DEFAULT_BRANCH, 1 << 40),
            ExtentId::new(DEFAULT_BRANCH + 1, 1 << 40),
        ];
        let discarded = HashSet::from([alive, far, other_branch]);
        assert_eq!(table.kill_orphans(DEFAULT_BRANCH, 20..21, &discarded), 2);
        let mut dead = table.dead_extents();
        dead.sort_by_key(|(extent, ..)| *extent);
        assert_eq!(
            dead,
            [
                (sought, FreeAfter::Now, 0, INVALID_OBJECT_ID),
                (far, FreeAfter::RecordedFlush(0), 0, INVALID_OBJECT_ID),
            ]
        );
        assert_eq!(table.kill_orphans(DEFAULT_BRANCH, 20..21, &discarded), 0, "killed twice");
        table.check();
    }

    /// Replay, which pins aren't in, kills an extent with its last deallocation, which nothing
    /// frees until the mount settles it; an allocation replayed in it after revives it, as far as
    /// that allocation reaches, as one of a shared extent that pins kept alive, live.
    #[fuchsia::test]
    fn replay_kills_an_emptied_extent_unsettled_and_revives_it() {
        let device = FakeDevice::new(64, BS as u32);
        let mut table = ExtentTable::default();
        let extent = ExtentId::new(DEFAULT_BRANCH, 5);
        let second = ExtentRange::new(extent, BS..2 * BS);
        commit(&mut table, &second, Applied::Replayed);
        let recount = table.recount(&second, -1, Applied::Replayed).expect("recount failed");
        assert_eq!(recount.records.0, [removed(extent, 2 * BS)]);
        assert_eq!(table.dead_extents(), [(extent, FreeAfter::Settling, 2 * BS, OWNER)]);
        assert!(table.start_freeing(u64::MAX, true).is_empty());
        table.check();

        commit(&mut table, &ExtentRange::new(extent, 0..BS), Applied::Replayed);
        assert!(table.dead_extents().is_empty());
        assert_eq!(table.alive().collect::<Vec<_>>(), [(extent, OWNER, BS)]);
        assert_eq!(table.extent_size(extent, &device), Some(BS));
        assert_eq!((table.alive_bytes(), table.dead_bytes()), (BS, 0));
    }

    /// What a mount found dead, however it died, is freed once a flush is recorded, which the
    /// journal's record of the discard comes before, if what replay discarded names it: an
    /// allocation discarded may be in it, which the next replay, after a crash before that record,
    /// reads and verifies (CI runs 41876989384 and 42196106048, TASK-210). Otherwise as it died,
    /// what replay killed at once, as a replay after reads what it says of it as this one did; and
    /// so orphans, found after replay as what it discarded settled (TASK-149.41). The extents of
    /// owners deleted wait for a recorded flush either way.
    #[fuchsia::test]
    fn a_mount_settles_what_it_found_dead_by_what_replay_discarded() {
        let device = FakeDevice::new(64, BS as u32);
        for named in [true, false] {
            let mut compacted = ExtentTable::default();
            let minted = compacted.mint(DEFAULT_BRANCH, &device);
            let [empty, deleted, replayed, orphan] =
                [10, 11, 12, 13].map(|s| ExtentId::new(DEFAULT_BRANCH, s));
            let mut loading = Loading::default();
            loading.add(&AllocatorKey::new(empty, 0..BS), 0, OWNER).expect("add failed");
            loading.add(&AllocatorKey::new(deleted, 0..BS), 1, OTHER_OWNER).expect("add failed");
            let mut table = ExtentTable::default();
            let deleting = HashSet::from([OTHER_OWNER]);
            let _ = table.load(loading, compacted.recorded_dying(), &deleting);
            let range = ExtentRange::new(replayed, 0..BS);
            commit(&mut table, &range, Applied::Replayed);
            let _ = table.recount(&range, -1, Applied::Replayed).expect("recount failed");
            let all = [minted, empty, deleted, replayed, orphan];
            let discarded = if named { HashSet::from(all) } else { HashSet::new() };
            table.settle_replay(&discarded);
            let sought = orphan.sequence..orphan.sequence + 1;
            assert_eq!(table.kill_orphans(DEFAULT_BRANCH, sought, &discarded), 1);

            let recorded = FreeAfter::RecordedFlush(0);
            let mount = if named { recorded } else { FreeAfter::Now };
            let mut dead = table.dead_extents();
            dead.sort_by_key(|(extent, ..)| *extent);
            assert_eq!(
                dead,
                [
                    (minted, mount, 0, INVALID_OBJECT_ID),
                    (empty, mount, BS, OWNER),
                    (deleted, recorded, BS, OTHER_OWNER),
                    (replayed, mount, BS, OWNER),
                    (orphan, mount, 0, INVALID_OBJECT_ID),
                ]
            );
            let mut now = table.start_freeing(u64::MAX, false);
            now.sort();
            let expected: &[ExtentId] =
                if named { &[] } else { &[minted, empty, replayed, orphan] };
            assert_eq!(now, expected);
            table.check();
        }
    }
}
