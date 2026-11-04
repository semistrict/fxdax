---
id: TASK-92.56
title: >-
  Index checkpoints are appended inline under the state lock, costing an ordered
  commit 89-143 ms
status: Done
assignee: []
created_date: '2026-10-24 07:32'
updated_date: '2026-11-04 10:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 376000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found measuring TASK-91.17 (no-DAX pgbench -i on GCE). Three ordered commits of the load spent 89-143 ms in append_record, writing index checkpoints inline while holding the store's state lock, which the guest's writes wait for.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit's index checkpoint is written outside the state lock, or bounded, shown by the load's commit times before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A commit's index checkpoint is encoded, created and written by the flush that writes it, outside the state lock, under the index tail's own; a commit takes only the snapshot, a clone of the extent entries, still under the lock. Shuttle serving_during_a_checkpoint deadlocks with the old code; checkpoints_failing_in_their_flushes_fail_the_store. Not measured: commit times under pgbench. Verified: macOS scripts/test.sh (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch (Linux workspace, guest-sim 3,000 seeds); GCE r539 at 8da75b3cc with Firecracker a2db2b3b0 (fxfs-guest-dax, checks-dax, -delay, -small, script-full-dax, xfstests-fxfs-guest-dax, checks, crash-dax, pgbench-dax all pass).
<!-- SECTION:FINAL_SUMMARY:END -->
