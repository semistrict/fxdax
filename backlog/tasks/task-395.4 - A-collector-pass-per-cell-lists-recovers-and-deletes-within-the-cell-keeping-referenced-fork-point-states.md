---
id: TASK-395.4
title: >-
  A collector pass per cell: lists, recovers and deletes within the cell,
  keeping referenced fork-point states
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.1
  - TASK-395.2
  - TASK-395.3
parent_task_id: TASK-395
priority: high
ordinal: 776000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Replaces the whole-bucket pass. A pass over one cell takes the cell's collector lease, lists the cell's prefix once (plus the relisting the spec requires), recovers the cell's live branches and the parent states at each referenced fork point, and compacts and deletes only under the prefix. Objects outside the prefix that the cell's branches name (inherited from other cells) are never candidates: the cells holding them keep them by reference. Today's pass costs 5+ full listings, a map of every object, and every branch recovered; per cell all of that is bounded by the cell.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A pass over a cell makes no LIST outside the cell's prefix and deletes nothing outside it; its memory and requests grow with the cell's objects, branches and references, not the bucket's (shown by a test counting requests and listed objects over a bucket of many cells)
- [ ] #2 A pass keeps everything the parent state at each referenced fork point needs (objects, moves ways, what recovering it reads), and a fork point's state shared by many references (a point cloned many times) is recovered once
- [ ] #3 A cell with no live branch and no references is reclaimed whole, but its cell object (kept, doc-7)
- [ ] #4 The whole-bucket pass and `gc/lease` are removed; `--dry-run`, `--mark-only`, `--branches` and `--delete-branch` work per cell or over all cells
- [ ] #5 Every existing collector test, the simulation, Shuttle and crash-round suites pass against the per-cell collector
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
