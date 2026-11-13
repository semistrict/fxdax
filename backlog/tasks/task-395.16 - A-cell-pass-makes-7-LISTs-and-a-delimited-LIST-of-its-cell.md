---
id: TASK-395.16
title: A cell pass makes 7 LISTs and a delimited LIST of its cell
status: To Do
assignee: []
created_date: '2026-11-13 23:50'
labels:
  - collector
  - performance
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 790000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured on ubu1 (run t395sweep, docs/measurements/ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e): every pass over a changed cell makes list 7, list-delimited 1 (70k LISTs a sweep of 10k cells). TASK-395.4's design says a pass lists its cell's prefix once, plus the relisting the spec requires. Find what each LIST is for, and whether one listing of the prefix (paged) can serve them, as LISTs are the costliest request at scale.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each LIST a pass makes is accounted for in doc-7, and those one listing of the prefix can serve are gone
- [ ] #2 A request-counting test pins a pass's LISTs per cell
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
