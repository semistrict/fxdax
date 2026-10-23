---
id: TASK-91.11
title: >-
  extentd: a WRITE's time grows with the store under pgbench without DAX (152 to
  211 us over five runs)
status: To Do
assignee: []
created_date: '2026-10-23 09:50'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 317000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 5 at a8e2e02 (2026-10-23), fxdax-nodax at 25 ms: five 60 s pgbench runs, WRITE requests flat (128-137k a run) while their total time grew 19.5 to 28.0 s, the average from 152 to 211 us (+39%), tps 1,293 to 1,095 (-15%); at 0 ms WRITE time 19.5 to 26.1 s, tps -10%. The store grows with every run, so something in extentd's write path scales with the store's size or history (its index, segments, open extents, the commit's seal of open extents...).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cost that grows is identified with a measurement of extentd's write path by phase
- [ ] #2 Run 5's average WRITE within 10% of run 1's (fxfs-guest-pgbench with five runs, or the sandbox team's harness)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
