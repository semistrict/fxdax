---
id: TASK-395.15
title: 'fxdax-gc''s sweep is fastest at 4 passes at once: 16 and 64 are slower'
status: To Do
assignee: []
created_date: '2026-11-13 23:50'
labels:
  - collector
  - performance
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 789000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1 run t395sweep on main 22602dbd1 (docs/measurements/ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e), 1.0M objects in 10k cells of a file bucket: first sweeps took 629 s at 1 pass at once, 320 s at 4, 492 s at 16, 395 s at 64. The sweep's passes share one task (TASK-395.5's notes), so their CPU work (digests, decoding, compaction) runs on one thread; the file bucket's own costs may add. Find the bottleneck (profile, perf on ubu1 under fxjob --bench) and make the sweep scale with passes at once, e.g. passes spawned on a multi-threaded runtime.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A profile of the 16-at-once sweep names where its time goes
- [ ] #2 Sweeps get faster up to at least 16 passes at once on ubu1's file bucket, measured with scripts/gc-sweep-host.sh
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
