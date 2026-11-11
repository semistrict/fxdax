---
id: TASK-350.4
title: 'Bench: fsync-heavy small transactions against a device that drops flushes'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 708000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandboxes' NBD server never sees a FLUSH, so a guest fsync ends in JuiceFS's memory buffer; every fxdax fsync is a guest Fxfs commit and an ordered COMMIT. pgbench measured 2,000 tps on Fxfs against 3,600-3,800 on ext4 with flushes dropped (docs/measurements/gce-pgbench-baselines-2026-11-08), guest CPU-bound.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench (scale 50, -c 4 -j 2), SQLite in rollback and WAL journal modes with synchronous=FULL, and apt installing a fixed package set, against the JuiceFS baseline
- [ ] #2 Reports tps / time, guest CPU split, and what each system has made durable when the run ends
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
