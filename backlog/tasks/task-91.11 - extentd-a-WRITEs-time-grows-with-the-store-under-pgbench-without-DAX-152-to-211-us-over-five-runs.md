---
id: TASK-91.11
title: >-
  extentd: a WRITE's time grows with the store under pgbench without DAX (152 to
  211 us over five runs)
status: Done
assignee: []
created_date: '2026-10-23 09:50'
updated_date: '2026-11-04 09:36'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04 on main cfd41e1a0: extentd's average WRITE, run 1 to run 5, over seven five-run no-DAX GCE runs: 64.7 to 65.5 us (+1.2%); main's three alone 64.9 to 67.1 us; not monotonic (runs 3-4 lowest, 51-57 us) (/Users/ramon/fxdax-gce/remeasure-20261104).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Not reproduced on main cfd41e1a0: extentd's average WRITE is 65 us in run 1 and in run 5, averaged over seven five-run no-DAX pgbench runs on GCE, where it grew from 152 to 211 us at round 5's build (a8e2e02). Nothing grows, so finding what grew (AC1) is moot.
<!-- SECTION:FINAL_SUMMARY:END -->
