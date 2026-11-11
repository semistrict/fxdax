---
id: TASK-350.4
title: 'Bench: fsync-heavy small transactions'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 10:59'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 708000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every fxdax fsync is a guest Fxfs commit and an ordered COMMIT; pgbench measured 2,000 tps on Fxfs against 3,600-3,800 on ext4 with flushes dropped (docs/measurements/gce-pgbench-baselines-2026-11-08), guest CPU-bound. On the JuiceFS baseline a guest flush reaches JuiceFS as an fsync of the image file with Firecracker cache_type Writeback, and is dropped with Unsafe: the loss is expected against Unsafe, and its size against Writeback is unknown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench (scale 50, -c 4 -j 2), SQLite in rollback and WAL journal modes with synchronous=FULL, and apt installing a fixed package set, against the JuiceFS baseline with cache_type Writeback and Unsafe
- [ ] #2 Reports tps / time, guest CPU split, and what each system has made durable when the run ends
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
