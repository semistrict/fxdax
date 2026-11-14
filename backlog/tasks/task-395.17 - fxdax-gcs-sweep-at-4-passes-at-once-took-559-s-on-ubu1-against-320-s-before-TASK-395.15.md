---
id: TASK-395.17
title: >-
  fxdax-gc's sweep at 4 passes at once took 559 s on ubu1, against 320 s before
  TASK-395.15
status: To Do
assignee: []
created_date: '2026-11-14 03:12'
labels:
  - collector
  - performance
dependencies: []
parent_task_id: TASK-395
priority: low
ordinal: 791000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1 run t395sweep2 (main 79c8249e1, docs/measurements/ubu1-gc-sweep-2026-11-14T0233Z-e344b176): first sweeps 602/559/190/112 s at 1/4/16/64 at once, against 629/320/492/395 s on c9b5ba549 (ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e). 16 and 64 got 2.6x and 3.5x faster, but 4 got 1.75x slower; one run each, so it may be noise. Re-measure 4 at once a few times on both commits; if it holds, find why (the multi-thread runtime's worker count, the 64 lock stripes, blocking-pool hand-offs).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 4 at once measured at least 3 times on each of c9b5ba549 and the current main, with the spread
- [ ] #2 If slower beyond the spread, the cause named and fixed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
