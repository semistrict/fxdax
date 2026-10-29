---
id: TASK-131.1
title: >-
  DAX regression at b5d3246: 720 against 839 tps, and ~21% lost over five
  pgbench runs: profile and bisect
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 472000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: DAX got 14% slower at b5d3246 (720 tps against 839), and loses ~21% over five runs; their guess is the seal correctness work. Profile a five-run pgbench-dax at b5d3246 and at the commit before the slowdown; bisect between; separate the first run's tps from the decline over runs (TASK-111.1's fault growth).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The commit or commits costing the tps are named, with profiles before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
