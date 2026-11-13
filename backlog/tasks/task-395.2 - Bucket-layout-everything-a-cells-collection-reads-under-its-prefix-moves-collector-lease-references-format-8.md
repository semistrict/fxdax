---
id: TASK-395.2
title: >-
  Bucket layout: everything a cell's collection reads under its prefix (moves,
  collector lease, references), format 8
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 09:43'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.1
parent_task_id: TASK-395
priority: high
ordinal: 774000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Moves objects live at `moves/<key>` (and `moves/moves/...`), so a cell's compacted objects are spread over top-level prefixes no cell's listing covers; the collector lease is one per bucket. For a cell to be collected from a listing of its own prefix, both move under it, and fork references get keys there too. A format change: format 8, which refuses 7 (AGENTS.md: no migrations).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Moves of a cell's objects, and moves of those moves, have keys under the cell's prefix; readers following an object's moves find them there
- [ ] #2 Each cell has a collector lease key under its prefix; `gc/lease` is gone
- [ ] #3 Fork references have keys under the parent's cell naming the parent branch, the fork point and the child (tenant, cell, branch), laid out so that one delimiter listing gives a branch's distinct fork points
- [ ] #4 `layout::kind` parses the new keys and `layout::owner` attributes every one to its cell; a bucket of format 7 is refused by name
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Format 8 is taken by TASK-395.7 (a point's record says whether it is shared): this task's layout change is format 9.
<!-- SECTION:NOTES:END -->
