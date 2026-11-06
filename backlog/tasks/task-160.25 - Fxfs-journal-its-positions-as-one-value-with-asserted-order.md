---
id: TASK-160.25
title: 'Fxfs journal: its positions as one value with asserted order'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-06 02:22'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 596000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The journal on a write-once device needs its own transaction, make_room, renew_tail, tail_unrecorded and nine watermark fields in Inner. Explains TASK-139, 60's asserts, 26.6.1, 149.3, and 153.13's own flush bug. Proposal: a JournalPosition holding the watermarks, debug-asserting their order (zeroed <= zero_offset, device-flushed <= written, synced <= begun). The structural option, one journal extent record per run of sequences, needs a prototype first.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 JournalPosition with asserted order
- [x] #2 Journal tests and model crash hunts pass with the asserts on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Refactor: a JournalPosition (journal/position.rs, fields private to it) holds the journal's positions: flushed_offset, valid_to, discard_offset, device_flushed_offset, device_flushed_end, needs_did_flush_device, barrier_wanted, barrier_written, flush_wanted, syncs_begun, synced, zero_offset, zeroed_to. Each change is a method named for the event (replayed, wrote, begin_sync, end_sync, want_written, device_flushed, superseded, zeroed, take_discard, take_flush_record); each ends with a debug-asserted check of the orders.
2. Orders: device_flushed_offset <= device_flushed_end <= flushed_offset; valid_to <= flushed_offset; discard <= valid_to; barrier_written <= flushed_offset; synced <= syncs_begun; zeroed_to < zero_offset; writes contiguous (each starts at flushed_offset); barrier_wanted, flush_wanted <= the writer's end; monotonic offsets; a super-block's journal start never before the last one's.
3. Replay sets device_flushed_end with device_flushed_offset (the mount's flush), so flush_device's max of the two goes.
4. Judge the structural option (one journal extent record per run of sequences) from the code; record it, no prototype.
5. Verify: cargo nextest -p fxfs -p fxfs-posix -p storage_device; model hunts (FXDAX_MODEL_CASES=3000, FXDAX_MODEL_FSCK=1); crates/fxfs-posix/mutants.sh; FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast; upstream-changes.py --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Design (2026-11-05)
JournalPosition (crates/fxfs/src/object_store/journal/position.rs, its own module so that its fields are private to it) holds the journal's 13 positions, which Inner had as fields: upstream's zero_offset, device_flushed_offset, needs_did_flush_device, discard_offset, valid_to, flushed_offset, and fxdax's zeroed_to, device_flushed_end, barrier_written, barrier_wanted, flush_wanted, syncs_begun, synced. Every change is a method named for what happened: replayed, flushed (a write of the journal), want_flushed, begin_sync, end_sync, device_flushed, take_discard, take_did_flush_device, superseded (a super-block written), zeroed; queries: flushing (what a device flush finds: durable, barriered, how far it makes durable), wants_barrier_after, zeroing, flush_is_wanted, flushed_offset, valid_to, next_sync, synced. Each method debug-asserts its own preconditions and ends with check(), the orders between the fields. Field names kept as upstream's, for porting its changes.
Simplified with it: replay sets device_flushed_end with device_flushed_offset (the mount's flush), so flush_device's max(device_flushed_offset, device_flushed_end) goes; superseded leaves no zero_offset when nothing's left to zero, so make_room's arm clearing it goes; begin_sync and end_sync assign where they took a max, the asserts saying why that's the same. Behaviour in release builds unchanged.

## Orders asserted
- zeroed_to < zero_offset <= device_flushed_end
- device_flushed_offset <= device_flushed_end <= flushed_offset (device-flushed <= written)
- discard_offset <= valid_to <= flushed_offset
- barrier_written <= valid_to
- synced <= syncs_begun; syncs end in the order they begin (end_sync: sync > synced)
- each write of the journal starts at flushed_offset and ends at a block (contiguous writes)
- barrier_wanted and flush_wanted no further than the journal's writer; barrier_wanted never goes back
- a device flush never goes back (device_flushed_offset), and is for no more than is written (flushing: offset <= flushed_offset, i.e. flush_device waited for the journal's write)
- a super-block's replay start never goes back, and is no further than a device flush made durable; the zeroing target never goes back
- replayed: valid_to <= the block-aligned end replay read

## The structural option: one journal extent record per run of sequences
Not prototyped; judged from the code. The journal's file has an extent record per 4 KiB block (EXTENT_SIZE): a 128 KiB chunk is 32 records and 32 Allocates, zeroing 2 MiB is 512 records and 512 Deallocates, which is why make_room slices (MAX_EXTENDED_PER_COMMIT, MAX_ZEROED_PER_COMMIT) and why TASK-139 happened. A run record (a file range to sequences s..s+n, a block each) would shrink only the object's side: the allocator keys each extent alone (one record per ExtentRange, each sealed and freed alone), so zeroing still deallocates per extent unless the allocator gains runs too, and a run must be minted at once (one counter, consecutive within one allocate) and splits at a renew_tail or a part zeroed. That reaches ExtentValue, the LSM merge of extent keys, the allocator's records and orphan search, fsck, replay's BootstrapObjectHandle, device_extents and renew_tail: Fxfs's most shared formats. The class it removes (the journal's own transaction outgrowing the room kept) is not the one this task's bugs show (orders between watermarks), and its asserts no longer panic (TASK-60). Judgement: not worth a prototype now. Reopen if the journal's own transactions show in a profile (commits waiting in make_room), or that class recurs.

## Verification at 73cf5cc23 (on origin/land-157), macOS, debug builds (asserts on)
- position.rs's tests: 2 of the API's sequences, and 13 should_panic tests, each order broken panicking where it says.
- Red with a bug back: flush_device without its wait for the journal's write: 13 journal tests panic at flushing()'s assert (a flush at X, the journal not written).
- cargo nextest run -p fxfs -p fxfs-posix -p storage_device: 882 of 882 before the last three asserts (zero_offset <= device_flushed_end, a super-block's start no further than durable, barrier_written <= valid_to); with them, 882 of 883: test_trim failed once (below).
- Model hunts: FXDAX_MODEL_CASES=3000, 33 of 33 (2,234 s); FXDAX_MODEL_FSCK=1 FXDAX_MODEL_CASES=200, 33 of 33 (127 s). No assert fired.
- crates/fxfs-posix/mutants.sh: --check 17 of 17 apply; 16 of 17 caught; truncate-gives-up-on-its-tail not caught, as on the base (TASK-160.20's notes).
- FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast: 2083 of 2083, 18 skipped.
- scripts/upstream-changes.py --check passes. clippy: nothing new in journal.rs or position.rs.
- Not run: GCE (Linux workspace, DoD #1), nor a guest kernel built with it.

## test_trim's flake: TASK-176
fsck of the snapshot replayed in test_trim's post-commit hook found a root-parent (owner 3) journal block missing from the allocator: 2 of 51 runs on this branch, 0 of 76 on origin/land-157 (stress runs, 4 at once, some during the hunt). No assert fired, and the change is behaviour-equivalent in release builds (each site checked); not significant, but not ruled out either. Filed with the evidence as TASK-176.

## Structure (AGENTS.md)
The bugs the task names (TASK-139, 60, 26.6.1, 149.3, 153.13) came from positions kept in step by hand at each site. Each now has one owner and its orders are checked as it changes; the next breach is a panic in a debug run at the change that made it, not a replay that fails later. The journal's own transaction (make_room) and its size class are left as they are; see the structural option above.

## TASK-182, found by TASK-160.23's fsck hunt (its task is on that branch, not this one), fixed here: fe1c465b8
The pinned sequence fails on this branch as on main, with no JournalPosition assert firing: the mismatch is between Root::journal_end, set before the cut, and what replay after it finds, which no in-memory position spans. Cause: a sync's barrier follows a flush of whole blocks, which may go past the sync's padding into blocks of later transactions; flush() records that write's end. A record begun in its last block and going on in the next, which the cut lost, stops replay at the record's start (307150), short of the end recorded (307200): refused, though the record's transaction was never whole, nor acknowledged. Replay now compares journal_end with the end of the block it read last (the reader verified it), or with where it discarded a transaction if it did, as before. Red then green: model test a_cut_with_fsck_after_every_transaction_replays_as_far_as_its_barrier. FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1: 34 of 34 (6,827 s). journal tests 84 of 84, test_a_journal_ending_before_its_synced_end_is_refused among them. Not the same cause as TASK-176 (test_trim's unlocked fsck).
After the rebase on origin/main (c919610e7) and these fixes: cargo nextest run -p fxfs -p fxfs-posix -p storage_device, 885 of 885.

Verified on main, 2026-11-05 (code of dde8d58d3; 752515020 adds backlog only): GCE r691, the Linux workspace, 2302 of 2303, the one failure TASK-187's a_store_after_a_workers_write_waits_for_one_lift (TASK-190; its fix 788e3b6b4 is on land-p, not main); GCE r690, the e2e suite (--tag e2e, 15 cases) jailed with the production seccomp filter, 15 of 15; CI 42137709782 on dde8d58d3 green (workspace 2303 of 2303, sim, shuttle, clippy scripts, guest, guest-sim); macOS FXDAX_TEST_PARTS=workspace scripts/test.sh at 752515020, 2118 of 2118; scripts/upstream-changes.py --check at 752515020 passes. AC #1: journal/position.rs on main, its 15 tests (13 should_panic, one per order broken) pass in r691. AC #2: r691 is a debug build (asserts on): fxfs's 79 journal tests and fxfs-posix's model tests (a_cut_with_fsck_after_every_transaction_replays_as_far_as_its_barrier among them) pass; the 3000-case and fsck model hunts with the asserts are recorded above. test_trim's flake was TASK-176, closed with this.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The journal's 13 positions are one JournalPosition whose methods debug-assert their orders; the run-of-sequences extent record judged not worth a prototype. On main: 8c27464ac (and 61acf7e40, TASK-182's replay fix, found by the same hunts). Verified: position and journal tests in GCE r691 (workspace 2302 of 2303, the failure TASK-190's), CI 42137709782, macOS workspace 2118 of 2118 at 752515020; model hunts as noted.
<!-- SECTION:FINAL_SUMMARY:END -->
