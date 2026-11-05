// Copyright 2026 The fxdax Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! fxdax: how far the journal has got, in each of the senses its writing, syncing and zeroing need,
//! as one value, changed only by what happens to the journal, a method each, after which the
//! orders between them are checked (in debug builds). Upstream keeps six of these as fields of the
//! journal's `Inner`. On a device whose extents are written once, a sync's barrier follows the
//! journal's write, syncs wait for each other, and the old journal is zeroed a slice a commit,
//! which took seven more, whose orders, kept by hand, explained a run of bugs (TASK-160.25).
//!
//! The orders, offsets in the journal:
//!
//! - `zeroed_to < zero_offset <= device_flushed_end`: the old journal is zeroed as far as the first,
//!   towards the second, the block where the last super-block but one started its replay: no
//!   further than the last starts it, which a flush of the device made durable.
//! - `device_flushed_offset <= device_flushed_end <= flushed_offset`: the device was last flushed
//!   for the journal as far as the first, which a DidFlushDevice record says, and the flushes made
//!   it durable as far as the second, the padding written after the first too.
//! - `discard_offset <= valid_to <= flushed_offset`: replay discards from the first; a replay would
//!   find the journal ending at the second.
//! - `barrier_written <= valid_to`: a barrier last followed a write that ended there.
//! - `flushed_offset`: the journal is written to its file as far as here, and each write goes on
//!   from it.
//! - `barrier_wanted` and `flush_wanted` are no further than the journal's writer, whose records
//!   aren't all written: a sync wants a barrier after its write as far as the first; something
//!   waits for the journal to be written as far as the second.
//!
//! And of syncs flushing the device, `synced <= syncs_begun`, each ending in the order it began.
//! No offset goes back, nor does where a super-block's replay starts.

use crate::object_store::journal::BLOCK_SIZE;
use std::ops::Range;

#[derive(Debug, Default)]
pub(super) struct JournalPosition {
    // The offset that we can zero the journal up to now that it is no longer needed. fxdax: none
    // once zeroed as far.
    zero_offset: Option<u64>,

    // fxdax: how far it has zeroed towards it, a slice a commit.
    zeroed_to: u64,

    // The journal offset that we most recently flushed to the device.
    device_flushed_offset: u64,

    // fxdax: how much of the journal had been written when the device was last flushed, which the
    // flush made durable: padding written after `device_flushed_offset` too, which needs no flush.
    device_flushed_end: u64,

    // If true, indicates a DidFlushDevice record is pending.
    needs_did_flush_device: bool,

    // If, after replaying, we have to discard a number of mutations (because they don't validate),
    // this offset specifies where we need to discard back to.  This is so that when we next replay,
    // we ignore those mutations and continue with new good mutations.
    discard_offset: Option<u64>,

    // The last offset that should be considered valid in the journal file.  Most of the time, this
    // will be the same as `flushed_offset` but at mount time, this could be less and will only be
    // up to the end of the last valid transaction; it won't include transactions that follow that
    // have been discarded.
    valid_to: u64,

    // fxdax: how far the journal was written when a barrier last followed its write.
    barrier_written: u64,

    // The last offset we flushed to the journal file.
    flushed_offset: u64,

    // fxdax: where a sync flushing the device padded the journal to: the flush of the journal
    // that writes as far ends with a barrier (`POST_BARRIER`), which is the sync's flush of the
    // device, one request rather than two.
    barrier_wanted: u64,

    // fxdax: the offset up to which something waits for the journal to be written.
    flush_wanted: u64,

    // fxdax: the syncs flushing the device begun, each numbered as it begins, and the latest done
    // (`Journal::next_sync`, `Journal::synced`): group commit, as ext4's jbd2 has it.
    syncs_begun: u64,
    synced: u64,
}

/// What a flush of the device for the journal as far as some offset finds (`flushing`).
#[derive(Clone, Copy, Debug)]
pub(super) struct DeviceFlush {
    /// Whether a flush of the device since the journal was written as far made it durable.
    pub durable: bool,
    /// Whether a barrier after the journal's write as far made it durable: then the device needs
    /// no flush of its own.
    pub barriered: bool,
    /// How far a flush of the device now makes the journal durable: as far as the barrier was
    /// after, or as far as the journal is written.
    pub end: u64,
}

impl JournalPosition {
    /// Replay read the journal as far as `end`, at a block, from which the journal goes on, and
    /// found it valid as far as `valid_to`: the next transaction's records discard what's between.
    /// A writable mount flushes the device, which makes it all durable.
    pub fn replayed(&mut self, end: u64, valid_to: u64) {
        debug_assert!(BLOCK_SIZE.is_aligned(end), "the journal goes on at {end}, not at a block");
        debug_assert!(valid_to <= end, "valid to {valid_to}, past where replay read, {end}");
        self.flushed_offset = end;
        self.device_flushed_offset = end;
        self.device_flushed_end = end;
        self.valid_to = valid_to;
        if valid_to < end {
            self.discard_offset = Some(valid_to);
        }
        self.check();
    }

    /// How far the journal is written to its file.
    pub fn flushed_offset(&self) -> u64 {
        self.flushed_offset
    }

    /// Where a replay would find the journal ending.
    pub fn valid_to(&self) -> u64 {
        self.valid_to
    }

    /// Something waits for the journal to be written as far as `offset`, its writer at `end`.
    /// Returns whether that is further than anything waited for before: the flush task is woken.
    pub fn want_flushed(&mut self, offset: u64, end: u64) -> bool {
        debug_assert!(offset <= end, "waits for the journal to {offset}, its writer at {end}");
        let further = offset > self.flush_wanted;
        self.flush_wanted = self.flush_wanted.max(offset);
        self.check();
        further
    }

    /// Whether something waits for the journal to be written further than it is.
    pub fn flush_is_wanted(&self) -> bool {
        self.flush_wanted > self.flushed_offset
    }

    /// Whether a write of `range` of the journal, the next, ends with a barrier: a sync wants one
    /// as far as it goes, which no barrier followed yet.
    pub fn wants_barrier_after(&self, range: &Range<u64>) -> bool {
        self.barrier_wanted > self.flushed_offset && range.end >= self.barrier_wanted
    }

    /// The journal is written over `range`, which goes on from what it was written as far as, and
    /// ends with a barrier if `barrier`.
    pub fn flushed(&mut self, range: Range<u64>, barrier: bool) {
        debug_assert_eq!(
            range.start, self.flushed_offset,
            "a write of the journal at {range:?}, not where it was written to"
        );
        debug_assert!(BLOCK_SIZE.is_aligned(range.end), "a write of the journal to {range:?}");
        self.flushed_offset = range.end;
        self.valid_to = range.end;
        if barrier {
            self.barrier_written = range.end;
        }
        self.check();
    }

    /// A sync flushing the device begins, the journal's writer padded to `end`: a barrier is wanted
    /// after its write as far. Returns the sync's number.
    pub fn begin_sync(&mut self, end: u64) -> u64 {
        debug_assert!(
            end >= self.barrier_wanted,
            "a sync at {end}, after one at {}: the journal's writer went back",
            self.barrier_wanted
        );
        self.barrier_wanted = end;
        self.syncs_begun += 1;
        self.check();
        self.syncs_begun
    }

    /// Sync number `sync` is done: syncs end in the order they begin.
    pub fn end_sync(&mut self, sync: u64) {
        debug_assert!(sync > self.synced, "sync {sync} done after sync {}", self.synced);
        self.synced = sync;
        self.check();
    }

    /// The number the next sync flushing the device takes.
    pub fn next_sync(&self) -> u64 {
        self.syncs_begun + 1
    }

    /// The number of the latest sync flushing the device that is done.
    pub fn synced(&self) -> u64 {
        self.synced
    }

    /// What a flush of the device for the journal as far as `offset` finds, as written now: the
    /// device's flush makes durable only what was written before it.
    pub fn flushing(&self, offset: u64) -> DeviceFlush {
        debug_assert!(
            offset <= self.flushed_offset,
            "a flush at {offset}, the journal not written"
        );
        let barriered = self.barrier_written >= offset;
        DeviceFlush {
            durable: self.device_flushed_end >= offset,
            barriered,
            end: if barriered { self.barrier_written } else { self.flushed_offset },
        }
    }

    /// The device is flushed for the journal as far as `offset`, as `flush` found it: a
    /// DidFlushDevice record is due.
    pub fn device_flushed(&mut self, offset: u64, flush: DeviceFlush) {
        debug_assert!(
            offset >= self.device_flushed_offset,
            "a flush of the device at {offset}, after one at {}",
            self.device_flushed_offset
        );
        debug_assert!(
            offset <= flush.end,
            "a flush of the device at {offset} durable to {flush:?}"
        );
        self.device_flushed_offset = offset;
        self.device_flushed_end = self.device_flushed_end.max(flush.end);
        self.needs_did_flush_device = true;
        self.check();
    }

    /// The offset a Discard record, due before the next transaction's records, discards from.
    pub fn take_discard(&mut self) -> Option<u64> {
        let discard = self.discard_offset.take();
        self.check();
        discard
    }

    /// The offset a DidFlushDevice record, due before the next transaction's records, says.
    pub fn take_did_flush_device(&mut self) -> Option<u64> {
        let offset = std::mem::take(&mut self.needs_did_flush_device);
        self.check();
        offset.then_some(self.device_flushed_offset)
    }

    /// A super-block is written whose replay starts at `start`, the last one's at `old_start`: the
    /// journal before there, its block, is zeroed.
    pub fn superseded(&mut self, old_start: u64, start: u64) {
        debug_assert!(
            old_start <= start,
            "a super-block's journal at {start}, the last's at {old_start}"
        );
        debug_assert!(
            start <= self.device_flushed_end,
            "a super-block's journal at {start}, durable only to {}",
            self.device_flushed_end
        );
        let zero_to = BLOCK_SIZE.align_down(old_start);
        debug_assert!(
            zero_to >= self.zero_offset.unwrap_or(self.zeroed_to),
            "zeroing to {zero_to}, having zeroed to {}, towards {:?}",
            self.zeroed_to,
            self.zero_offset
        );
        self.zero_offset = (zero_to > self.zeroed_to).then_some(zero_to);
        self.check();
    }

    /// Where the old journal is zeroed to next: at most `max` of what's left.
    pub fn zeroing(&self, max: u64) -> Option<Range<u64>> {
        self.zero_offset.map(|end| self.zeroed_to..end.min(self.zeroed_to + max))
    }

    /// `range` of the old journal is zeroed, as `zeroing` said.
    pub fn zeroed(&mut self, range: Range<u64>) {
        debug_assert_eq!(range.start, self.zeroed_to, "zeroed {range:?}, not from where it was");
        debug_assert!(
            self.zero_offset.is_some_and(|end| range.end <= end),
            "zeroed {range:?}, past {:?}",
            self.zero_offset
        );
        self.zeroed_to = range.end;
        if self.zero_offset == Some(range.end) {
            self.zero_offset = None;
        }
        self.check();
    }

    /// Where the old journal is zeroed to, if anywhere.
    #[cfg(test)]
    pub fn zero_offset(&self) -> Option<u64> {
        self.zero_offset
    }

    /// The orders between them (the module's comment).
    fn check(&self) {
        if let Some(zero_offset) = self.zero_offset {
            debug_assert!(self.zeroed_to < zero_offset, "{self:?}: zeroed past where it zeroes to");
            debug_assert!(
                zero_offset <= self.device_flushed_end,
                "{self:?}: zeroes what isn't durable"
            );
        }
        debug_assert!(
            self.device_flushed_offset <= self.device_flushed_end,
            "{self:?}: the device flushed for more than it made durable"
        );
        debug_assert!(
            self.device_flushed_end <= self.flushed_offset,
            "{self:?}: durable further than written"
        );
        if let Some(discard_offset) = self.discard_offset {
            debug_assert!(discard_offset <= self.valid_to, "{self:?}: discards past where valid");
        }
        debug_assert!(self.valid_to <= self.flushed_offset, "{self:?}: valid further than written");
        debug_assert!(
            self.barrier_written <= self.valid_to,
            "{self:?}: a barrier after more than is valid"
        );
        debug_assert!(self.synced <= self.syncs_begun, "{self:?}: more syncs done than begun");
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceFlush, JournalPosition};

    // A block of the journal's.
    const B: u64 = 4096;

    // A journal a mount replayed as far as `end`, and flushed.
    fn replayed_to(end: u64) -> JournalPosition {
        let mut position = JournalPosition::default();
        position.replayed(end, end);
        position
    }

    #[test]
    fn test_a_journal_replayed_written_and_synced() {
        let mut position = JournalPosition::default();
        position.replayed(2 * B, B + 100);
        assert_eq!(position.take_discard(), Some(B + 100));
        assert_eq!(position.take_discard(), None);
        assert_eq!(position.valid_to(), B + 100);
        assert!(position.flushing(2 * B).durable, "the mount flushed what replay read");

        // A sync padded to 4 blocks, written in two: the barrier follows the second.
        assert!(position.want_flushed(4 * B, 4 * B));
        assert!(!position.want_flushed(3 * B, 4 * B), "waited for already");
        assert_eq!(position.begin_sync(4 * B), 1);
        assert_eq!(position.next_sync(), 2);
        assert!(!position.wants_barrier_after(&(2 * B..3 * B)));
        position.flushed(2 * B..3 * B, false);
        assert!(position.flush_is_wanted());
        assert!(position.wants_barrier_after(&(3 * B..4 * B)));
        position.flushed(3 * B..4 * B, true);
        assert!(!position.flush_is_wanted());
        assert_eq!((position.flushed_offset(), position.valid_to()), (4 * B, 4 * B));

        let flush = position.flushing(4 * B - 10);
        assert_eq!((flush.durable, flush.barriered, flush.end), (false, true, 4 * B));
        position.device_flushed(4 * B - 10, flush);
        assert_eq!(position.take_did_flush_device(), Some(4 * B - 10));
        assert_eq!(position.take_did_flush_device(), None);
        position.end_sync(1);
        assert_eq!(position.synced(), 1);

        // The padding written after the flush's offset is durable with it.
        assert!(position.flushing(4 * B).durable);
        position.flushed(4 * B..5 * B, false);
        let flush = position.flushing(5 * B);
        assert_eq!((flush.durable, flush.barriered, flush.end), (false, false, 5 * B));
    }

    #[test]
    fn test_the_old_journal_is_zeroed_in_slices() {
        let mut position = replayed_to(8 * B);
        assert_eq!(position.zeroing(B), None);
        position.superseded(3 * B + 100, 5 * B);
        assert_eq!(position.zero_offset(), Some(3 * B));
        assert_eq!(position.zeroing(2 * B), Some(0..2 * B));
        position.zeroed(0..2 * B);
        // A super-block written meanwhile zeroes further.
        position.superseded(5 * B, 6 * B);
        assert_eq!(position.zeroing(2 * B), Some(2 * B..4 * B));
        position.zeroed(2 * B..4 * B);
        assert_eq!(position.zeroing(2 * B), Some(4 * B..5 * B));
        position.zeroed(4 * B..5 * B);
        assert_eq!(position.zeroing(2 * B), None);
        // One whose last started in the block zeroed to has nothing to zero.
        position.superseded(6 * B - 1, 6 * B);
        assert_eq!(position.zeroing(2 * B), None);
    }

    // Each order broken panics in a debug build, where the module's comment says it can't be.

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "not where it was written to")]
    fn test_a_write_not_after_the_last_panics() {
        JournalPosition::default().flushed(B..2 * B, false);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "durable further than written")]
    fn test_a_flush_durable_past_what_is_written_panics() {
        let flush = DeviceFlush { durable: false, barriered: false, end: B };
        JournalPosition::default().device_flushed(B, flush);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "a flush of the device at 4096, after one at 8192")]
    fn test_a_flush_of_the_device_going_back_panics() {
        let mut position = replayed_to(2 * B);
        let flush = position.flushing(B);
        position.device_flushed(B, flush);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "valid to 8192, past where replay read, 4096")]
    fn test_valid_past_what_replay_read_panics() {
        JournalPosition::default().replayed(B, 2 * B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "waits for the journal to 8192, its writer at 4096")]
    fn test_waiting_past_the_writer_panics() {
        JournalPosition::default().want_flushed(2 * B, B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "the journal's writer went back")]
    fn test_a_sync_before_the_last_panics() {
        let mut position = JournalPosition::default();
        position.begin_sync(2 * B);
        position.begin_sync(B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "sync 1 done after sync 2")]
    fn test_syncs_ending_out_of_order_panics() {
        let mut position = JournalPosition::default();
        position.begin_sync(0);
        position.begin_sync(0);
        position.end_sync(2);
        position.end_sync(1);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "more syncs done than begun")]
    fn test_a_sync_done_never_begun_panics() {
        JournalPosition::default().end_sync(1);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "a super-block's journal at 4096, the last's at 8192")]
    fn test_a_super_block_starting_before_the_last_panics() {
        JournalPosition::default().superseded(2 * B, B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "a super-block's journal at 4096, durable only to 0")]
    fn test_a_super_block_starting_past_what_is_durable_panics() {
        JournalPosition::default().superseded(0, B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "zeroing to 4096, having zeroed to 0, towards Some(8192)")]
    fn test_zeroing_less_than_before_panics() {
        let mut position = replayed_to(2 * B);
        position.superseded(2 * B, 2 * B);
        position.superseded(B, 2 * B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "zeroed 0..12288, past Some(8192)")]
    fn test_zeroing_past_where_it_zeroes_to_panics() {
        let mut position = replayed_to(2 * B);
        position.superseded(2 * B, 2 * B);
        position.zeroed(0..3 * B);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "zeroed 4096..8192, not from where it was")]
    fn test_zeroing_not_from_where_it_was_panics() {
        let mut position = replayed_to(2 * B);
        position.superseded(2 * B, 2 * B);
        position.zeroed(B..2 * B);
    }
}
