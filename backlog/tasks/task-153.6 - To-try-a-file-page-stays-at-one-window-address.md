---
id: TASK-153.6
title: 'To try: a file page stays at one window address'
status: Done
assignee: []
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 02:10'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 550000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A page location lives in five places since fxfs_dax_move broke dax.c header promise, explaining 13 bugs (TASK-121, 123, 124, 125, 128, 133, 79.106, patch 0008, dce4f144b, candidates 1, 2, 3, 5), and evacuation exists to put pages back. Moves would remap a page at its fixed address, batched with MAP_MANY, which brings back the MAP cost regions were added to avoid. dead-ends/batched-evacuation.md: the cost is per page, so this is what could make evacuation cheaper per page. Prototype first of the two big ones, as it may remove much of the other.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A prototype
- [ ] #2 pgbench-dax and the write breakdown measured against it, several rounds a side
- [ ] #3 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Done: a file's page stays at one window address (fafefbac7): pages no longer move to their staging extent's region, nor does an evacuation move them back, 2,300 lines fewer; a write(2) takes no MAP of its own, its pages lagging until first use maps the chunk's together (106b6128d). Same host: pgbench 1,071 against 1,011 tps, rand-write 18,151 against 3,534 IOPS; guest/sim interleave and failures seeds pass; load_during_move's hang (r496) since found to be the test's own and fixed (r528, 20 of 20).
<!-- SECTION:FINAL_SUMMARY:END -->
