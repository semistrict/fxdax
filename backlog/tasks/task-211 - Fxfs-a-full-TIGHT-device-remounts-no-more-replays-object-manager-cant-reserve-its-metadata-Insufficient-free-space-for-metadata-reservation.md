---
id: TASK-211
title: >-
  Fxfs: a full TIGHT device remounts no more: replay's object manager can't
  reserve its metadata (Insufficient free space for metadata reservation)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 11:43'
updated_date: '2026-11-06 12:39'
labels:
  - bug
  - fxfs
dependencies: []
priority: high
type: bug
ordinal: 650000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 1,000-case model hunt of TASK-210 (2026-11-06, macOS, on 36cd32147 with and without TASK-210's fix, so not that change). A sequence on a TIGHT (2048-block) device ends with a clean remount that fails: "at the end: opening: Journal replay failed: Failed to complete replay for object manager: Insufficient free space for metadata reservation.: Failed to reserve 1607432 - 692548 = 914884 bytes, free=827392". A filesystem that unmounted cleanly then mounts no more. Reproduce, deterministic, 0.45 s, in crates/fxfs-posix/src/model_tests.rs: Setting::awkward(0, TIGHT).with_graveyard_of_seed().holds(vec![Op::Create(2), Op::Create(0), Op::Fill { file: 5, page: 0, pages: 1329, stride: 2, byte: 0 }, Op::Fill { file: 0, page: 0, pages: 1885, stride: 2, byte: 0 }, Op::Fill { file: 0, page: 0, pages: 45, stride: 1, byte: 0 }, Op::Punch { file: 0, page: 0, pages: 1 }]). Main's commits since 1fefc017c touch only xattrs in fxfs and fxfs-posix, which the sequence doesn't use, so it is likely older and rare. Related: TASK-26.6.1, TASK-58, TASK-60 (reservation exhaustion).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The sequence pinned as a model test, failing before the fix
- [x] #2 Its cause named: why replay needs more metadata reservation than a full device leaves after a clean unmount
- [x] #3 A full device remounts: the pinned test passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Pin the sequence (model_tests.rs), red on main.
2. Print the reservation terms at close and at mount (temporary): find whether the mount asks more than the live filesystem held, or finds less free.
3. Fix the accounting at its root, with a class check and an allocator test, each red with the fix out.
4. Suites and 1,000-case hunts, plain and with FXDAX_MODEL_FSCK=1, on macOS and Linux (Docker).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## Reproduction
Seed 0, TIGHT, 6 ops, from TASK-210's hunt. Pinned: model_tests.rs a_full_device_unmounted_cleanly_mounts_again, red on main (1fefc017c and 36cd32147): "at the end: opening: … Insufficient free space for metadata reservation: Failed to reserve 1607432 - 692548 = 914884 bytes, free=827392".

## Cause
The mount asks exactly what the live filesystem had reserved (required 1607432 - borrowed 692548), so the mount is right. The live filesystem already held more than its device: at close, with the metadata reservation 914884 still held, used = 8476100 of a device of 8388608. The first transaction that took it past the disk committed Allocate of staging extent …805 at 208896..393216. used grew 208896 for 184320 bytes allocated, because the extent counts whole (ExtentTable::commit: alive_bytes += range.end - info.size) and its first 208896 bytes were nobody's. Just before, a transaction holding the shared allocation 0..208896 of that extent had dropped. Allocator::drop_mutation (allocator.rs, JournalingObject impl) gave the range back to the transaction's reservation and the free pool, as upstream Fxfs does, since a block allocator can reuse freed blocks. In fxdax an extent is written once and taken whole: the range stays taken by the staging extent (ExtentTable::stage hands out ranges past staging.next, which never goes back), and ExtentTable::drop_allocation only unpinned it. So the bytes were free for another to take, a reservation (here the metadata reservation) took them, and the next commit in the extent counted them again: double counting, invisible until the next mount reserves honestly.

## Fix
ExtentTable::drop_allocation: when the extent lives on (pinned by staging or by other allocations), it takes the dropped range from then on, as a committed one would. ExtentState::Pending gains a size, and ExtentState::reach grows any state's size. It returns Dropped { records, kept }, kept being the bytes of the range newly taken. Allocator::drop_mutation treats those as a commit of the transaction's reservation (Reservation::commit) and gives back only the rest. Without a reservation of its own, they move from uncommitted to alive. If the extent dies with its last pin, what it took goes dead with it and is freed (kept 0), as an unshared extent dropped is.

## Blind spot and tests (AGENTS.md)
- Why green: nothing checked that a live filesystem holds no more than its device. The mismatch shows only on the next mount, only if the device is nearly full (TIGHT) and a shared allocation dropped (a fill refused) before another in the same staging extent committed.
- Class check: FxFilesystem::close, once synced, debug-asserts that used (with the metadata reservation, which the next mount reserves again) fits the device. Every fxfs test and every model sequence and cut closes, so all of them check it. With the fix out, the pinned test fails at close: "the filesystem holds 8476100 bytes of a device of 8388608".
- Allocator test: test_a_shared_allocation_dropped_stays_taken. A reservation takes all that is free, then a shared allocation drops and the next in its extent commits; used must stay within the device. It fails with the fix out (16781312 bytes held of 16777216), and with either half of the fix out (counting the range at the drop, or committing it from the reservation).
- Extent table tests: a_shared_extent_takes_what_was_dropped_in_it_once (drop then commit, and commit then drop: counted once); pins_hold_an_emptied_shared_extent and a_staging_extent_nothing_committed_in_dies_unrecorded updated (a dropped range is taken until the extent dies).
- Looked around: callers of deallocate use its length for file sizes and project usage, not reservations, so no second leak there. Commit past ranges still in flight counts them twice for a while (alive and uncommitted). That errs safe (pessimistic), and it is why the check is at close, with nothing in flight. Filed TASK-213: a mount sizes an extent from its records, so a dropped range past the last committed one is counted live but not after a remount.
- Structure: the extent table now owns what a dropped range takes, and the allocator keeps the reservation arithmetic beside it. A deeper change would have staging count its ranges as it hands them out, which ends the double counting while ranges are in flight. It touches every shared allocation's accounting and the owners' limits, so I left it, as the user's to weigh.

## Verification (on 9bdc1c132, TASK-210's head)
- macOS: cargo nextest run -p fxfs -p fxfs-posix 849 of 849; whatever_a_vfs_asks FXDAX_MODEL_CASES=1000: passed (2,097 cuts, 3,812 of replays, 482 s); with FXDAX_MODEL_FSCK=1: passed (2,094 cuts, 3,906 of replays, 938 s).
- Linux (Docker arm64, rust:1): the same suite 849 of 849; hunt 1,000 cases passed (2,101 cuts, 3,627 of replays, 406 s); fsck hunt passed (2,142 cuts, 3,834 of replays, 769 s). With the close check in, every case of these hunts checked that its filesystem held no more than its device.
- scripts/upstream-changes.py --check passes. Workspace suite and GCE not run (DoD #1 open); the change is in fxfs alone.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A full device that unmounted cleanly failed to remount because its live filesystem had held more than the device. A shared (staging) allocation that dropped gave its range back to its reservation and the free pool, as upstream Fxfs does. But in fxdax the staging extent is written once and keeps that range, and the next commit in it counted the range again. The mount then reserved honestly and found too little. Now a dropped range in a shared extent that lives on is taken by it (ExtentTable::drop_allocation, ExtentState::reach) and committed from the transaction's reservation (Allocator::drop_mutation). FxFilesystem::close debug-asserts that a synced filesystem holds no more than its device, which every test and model cut now checks. Tests red with the fix out: the pinned model sequence, an allocator test (red with either half of the fix out too), and extent-table tests. Verified: fxfs and fxfs-posix suites and 1,000-case hunts, with and without FXDAX_MODEL_FSCK=1, on macOS and Linux (Docker). GCE not run. Filed TASK-213.
<!-- SECTION:FINAL_SUMMARY:END -->
