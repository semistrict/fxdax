---
id: TASK-350.4
title: 'Bench: fsync-heavy small transactions'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 11:01'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 708000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every fxdax fsync is a guest Fxfs commit and an ordered COMMIT; pgbench measured 2,000 tps on Fxfs against 3,600-3,800 on ext4 with flushes dropped and 1,535 with each flush a host fsync (docs/measurements/gce-pgbench-baselines-2026-11-08), guest CPU-bound.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench (scale 50, -c 4 -j 2), SQLite in rollback and WAL journal modes with synchronous=FULL, and apt installing a fixed package set, against the loop-device baseline (TASK-350.1) with cache_type Writeback and Unsafe
- [ ] #2 Reports tps / time and guest CPU split
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
