---
id: TASK-109.1
title: >-
  Guest DAX: the evacuation after every seal maps most written pages a second
  time (77k of 120k MAPs per pgbench run)
status: In Progress
assignee: []
created_date: '2026-10-25 23:26'
updated_date: '2026-10-25 23:42'
labels: []
dependencies: []
parent_task_id: TASK-109
priority: high
ordinal: 410000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-91.10's evacuation (fxfs_dax_evacuate_work) moves every sealed staging extent's pages into its range's own space with MAP_MANY after each seal; the writes after the seal restage. Under an fsync per transaction a page is mapped once as staging and again as an evacuee: per 60 s pgbench run (sandbox team, b746104) 77,010 evacuate and 33,233 restage MAPs of 120,091, and fxfs_dax_evacuee_gather, walking every range of each affected file page by page, is 4% of guest CPU samples. Each MAP costs the VMM ~175 us of host kernel time (TASK-91.16), so the evacuation alone is a large part of DAX's gap to no DAX (TASK-109).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Evacuate MAPs per pgbench-dax 60 s run cut to under a tenth of today's, with the window still not filling (window_full and regions pruned reported before and after)
- [ ] #2 fxfs_dax_evacuee_gather no longer walks a file's every range page by page per seal
- [ ] #3 fxfs-guest-checks-dax (regions_evacuated, evacuation_rewrites, seal_race) green on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Hypothesis: much of the evacuation maps pages rewritten right after (the WAL's tail page, written at every commit, moves to the next staging region at the next write, so its evacuation MAP is wasted), and pages written once (WAL) are rarely read through the window again. Experiment exp/evacuate-delay (5492cf523, not for main): fxfs.dax_evacuate_delay_ms, the evacuation skipping regions sealed less than that long ago. r112: pgbench-dax, 1 VM, 0 / 200 / 2000 ms.
<!-- SECTION:NOTES:END -->
