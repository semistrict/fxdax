---
id: TASK-164.2
title: 'Guest: DAX writes copy on write, allocated at writeback'
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
updated_date: '2026-11-04 16:35'
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
- [ ] #1 A dirty set with reservations; writeback by fxfs_write_many from the window, committing after the bytes
- [ ] #2 Rebinding to the new sealed extent under the semaphore held exclusively; the budget throttles under private_max; holes as zeros; truncate, punch, clone and freeze
- [ ] #3 The DAX and crash suites pass with it on
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: landed behind fxfs.dax_cow with the six fixes guest/sim found; GCE r547 with it on: crash-dax, pgbench-dax-cow, pgbench-dax-cow-forked, bigmap-dax-cow pass; fxfs-guest-dax (small_files_share: 37 MAPs, packing at writeback to check) and checks-dax (a deadlock: fxfs.dax_rebind_now's setter flushes the rebind work holding kernel_param_lock while a check pauses it) left for batch 2026-11-04e, which also deletes writable DAX and the flag (the user's decision).
<!-- SECTION:NOTES:END -->
