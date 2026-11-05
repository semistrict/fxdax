---
id: TASK-164.2
title: 'Guest: DAX writes copy on write, allocated at writeback'
status: Done
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-05 04:54'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 612000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Behind fxfs.dax_cow (doc-13 stage 2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A dirty set with reservations; writeback by fxfs_write_many from the window, committing after the bytes
- [x] #2 Rebinding to the new sealed extent under the semaphore held exclusively; the budget throttles under private_max; holes as zeros; truncate, punch, clone and freeze
- [x] #3 The DAX and crash suites pass with it on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: landed behind fxfs.dax_cow with the six fixes guest/sim found; GCE r547 with it on: crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass; fxfs-guest-dax (small_files_share: 37 MAPs, packing at writeback to check) and checks-dax (a deadlock: fxfs.dax_rebind_now's setter flushes the rebind work holding kernel_param_lock while a check pauses it) left for batch 2026-11-04e, which also deletes writable DAX and the flag (the user's decision).

Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Copy-on-write DAX writes: the iomap's dirty set with reservations, writeback by fxfs_write_many committing after the bytes, rebind once sealed, the budget under private_max, holes as zeros (37c331a6c and the batch-4 commits). AC2's pieces: guest/sim's scenarios and interleave (private_max, rebind, punch, clone, truncate) and checks-dax's freeze, punch, clone and truncate checks. Verified on main 09b36991a: macOS workspace 2052/2052 + Shuttle; CI 41917089568 (6 jobs); GCE r553 Linux workspace 2232/2232; r555 jailed with the production seccomp filter: fxfs-guest-dax, checks-dax, checks-dax-delay, crash-dax, pgbench-dax, migrate-tenant-dax pass; r554: xfstests-fxfs-guest-dax, pgbench-dax-forked, bigmap-dax, checks-dax-small pass; guest/sim zloop 3000 seeds; TLA+ 24/24; scripts/upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
