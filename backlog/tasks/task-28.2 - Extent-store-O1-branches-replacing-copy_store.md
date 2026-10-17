---
id: TASK-28.2
title: 'Extent store: O(1) branches, replacing copy_store'
status: To Do
assignee: []
created_date: '2026-10-17 01:31'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: feature
ordinal: 82000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A fork copies every file of the store today (extent_store::copy_store, used by fxdax image clones and extentd forks). The remote layout in doc-7 needs lineage: a child records its parent and fork point and reads inherited extents through the branch that minted them, and a free of an inherited extent drops only the freeing branch claim. doc-7 decision 10 puts this before uploads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Forking a store takes constant time and space whatever the store holds, shown by a test
- [ ] #2 Parent and child read every extent the parent had at the fork; frees after the fork by either do not affect the other, and an extent both free is reclaimed, shown by tests
- [ ] #3 A fork seals the parent open extents
- [ ] #4 fxdax image clones and extentd forks use it, and their tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
