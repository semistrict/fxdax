---
id: TASK-67.1
title: >-
  A syncfs after fsyncs returns before their commits are durable: a lost host
  loses what syncfs acknowledged
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 07:53'
updated_date: '2026-11-07 23:54'
labels: []
dependencies: []
parent_task_id: TASK-67
priority: high
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67), GCE r750 round 3 (run FXDAX_SEED=17637234348586437544, FXDAX_SOAK_LOAD_SECS=60; round seed 12905498833013719694): the host lost 18 s into the load, slot 2's writers had syncfs return for files 0-288 and 0-286, and the store taken again from the bucket held 0-283 and 0-282. doc-12 promises a sync survives a lost host. The guest's durability is a mode, not a request: commits made while a sync holds `Durability` are COMMIT_DURABLE (guest/rust/fxfs-linux/src/device.rs:615-668), and fxfs-posix's `sync` holds it around `FxFilesystem::sync`; but the journal counts itself durable as far as any flush took it (`JournalPosition::flushing`, crates/fxfs/src/object_store/journal/position.rs:195-204), an fsync's ordered one too, and `Journal::flush_device` (journal.rs:1997) then asks the device nothing. So a syncfs finding nothing new since the last fsync made nothing durable, and returned before the fsyncs' commits were uploaded.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A sync ends with a durable commit of the device whatever the journal flushed, shown by a test that fails without it
- [x] #2 fxfs-guest-soak runs host-loss rounds after syncfs without losing a file a syncfs acknowledged
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A test in fxfs-posix with a platform counting the device's barriers made while a sync wants durability: fsync, then sync, must make one (red).
2. Platform-independent fix: Fxfs::sync flushes the device once the journal's sync is done, while the durability guard lives.
3. fxfs-posix suite, macOS workspace, GCE Linux workspace, and the soak's host-loss rounds.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in crates/fxfs-posix/src/lib.rs (Fxfs::sync): after FxFilesystem::sync, while the platform's durability guard lives, a flush of the device, which the guest's device makes a COMMIT_DURABLE (guest/rust/fxfs-linux/src/device.rs:1184-1190, durable(false) while the guard is held). Where the journal's own flush was durable, it finds nothing left and costs a round trip.

Test: fxfs-posix tests::syncs_make_what_fsyncs_ordered_durable, a platform whose Durable guard is counted and a FakeDevice whose op callback counts Flush and PostBarrier made while it is held: red before (round 0: the sync after an fsync made nothing durable), green after; fxfs-posix 68/68 on macOS; macOS workspace 2032/2032; GCE r756 Linux workspace 2223/2223 and its other parts.

Blind spot: no test told an ordered commit from a durable one at a sync. fxfs-posix's test platforms have Durable = (); the guest crash cases lose the VM or VMM, which keep ordered commits, and fxfs-guest-crash-dax-daemon-killed holds fsyncs to a prefix and nothing to syncfs; no case lost a host. The class test is the soak's host-loss rounds, which hold every file a syncfs acknowledged (soak::kept): r750 round 3 red; r754 round 2 (reforks, VM crashes, an upload failed, host lost at 141 s) green with the fix, and every host-loss round of r755-r757 since.

Looking around: the guest's freeze makes the same guard, and ends in an explicit COMMIT_DURABLE (Freezer::freeze), so held. Unmount's close commit is always durable. Nothing else takes Platform::durable.

Structure: durability is a mode commits made under a guard take, not a request; a sync that makes no commit makes nothing durable. The fix makes the request explicit at the one durable sync. Making it impossible would have the journal know the device's two levels (an ordered flush counted durable only for ordering), which is Fxfs's upstream journal: left, the explicit flush instead.

Finalized in the backlog sweep: its commits are on origin/main. DoD #1 by Main CI 42423368353 at 878aa8848 (which holds this work) green, macos (workspace) and test (workspace) among it; scripts/upstream-changes.py --check passes on main (2026-11-08 sweep).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed on main (8b8348619): Fxfs::sync ends with a flush of the device while the platform's durability guard lives (crates/fxfs-posix/src/lib.rs:1727), which the guest's device makes a COMMIT_DURABLE, so a syncfs after fsyncs makes their ordered commits durable and uploaded. syncs_make_what_fsyncs_ordered_durable (crates/fxfs-posix/src/tests.rs:808) red before, green after; fxfs-guest-soak's host-loss rounds kept every file a syncfs acknowledged since (GCE r754-r757, per the notes). Main's CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
