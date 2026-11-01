---
id: TASK-153.6
title: 'To try: a file page stays at one window address'
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
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
