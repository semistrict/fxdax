---
id: TASK-28.10
title: 'Garbage collector: fxdax-gc'
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
labels: []
milestone: m-5
dependencies:
  - TASK-28.1
  - TASK-28.2
  - TASK-28.6
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: feature
ordinal: 90000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 Garbage collection: the only thing that deletes. Marks from every live branch head and fork points, compacts mostly-dead segments through moves records, deletes orphans, and finishes branch deletions, coordinating through object storage alone (doc-2 requirement 13).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 It reclaims freed extents and compacts mostly-dead segments, and readers on other hosts with older indexes still read moved extents
- [ ] #2 It holds a lease; two collectors, or one crashing or fenced at any step, lose nothing
- [ ] #3 It passes the simulation (TASK-28.1) with the real code in place of the model
- [ ] #4 A collection that cannot read a branch metadata stops rather than deleting
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
