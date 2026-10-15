---
id: TASK-26.18
title: 'Guest Fxfs: every data and metadata operation within 2x of ext4 on DAX pmem'
status: In Progress
assignee: []
created_date: '2026-10-15 08:18'
labels: []
dependencies: []
parent_task_id: TASK-26
priority: high
type: task
ordinal: 54000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The goal (user, 2026-10-15): fxfs-guest-dax within 2x of ext4-dax-pmem on GCE for every operation in test_fs_bench.rs. As of 1df9892 every data operation is under 2x. The unlink family and mkdir-tree are not: per op, Fxfs commits each metadata change (about 4 in-memory LSM mutations plus a journal record) while the VFS holds the directory lock, and host requests stall the writing thread (a MAP takes about 0.85 ms). Profiles and A/B runs are in docs/measurements and DONE.md. Measure on GCE only (scripts/bench-gce.sh); Lima numbers swing 2x.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A GCE run of all benchmark groups (3 iterations) shows every operation of fxfs-guest-dax within 2x of ext4-dax-pmem
- [ ] #2 The run is saved in docs/measurements and the history updated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
