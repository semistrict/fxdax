---
id: TASK-26.10
title: 'Local extent store: segments on disk; fxdax''s images become extent stores'
status: To Do
assignee: []
created_date: '2026-10-13 22:51'
labels: []
milestone: m-3
dependencies:
  - TASK-26.3
parent_task_id: TASK-26
ordinal: 46000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With Fxfs storing extents only (TASK-26.3), fxdax's image files must become an extent store on local disk, and the host's device (TASK-26.4) needs the same store. doc-4's host storage: sealed extents under 4 MiB packed in commit order into segment files of about 16-64 MiB, extents of 4 MiB and more as files of their own, an index from extent ID to location, and cleaning of mostly-dead segments. Milestone 3 uploads the same layout to object storage, so the layout should be the one doc-2 gives for it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A local extent store keeps extents in segment files and files of their own, with an index, and implements the extent-store interface including the root record
- [ ] #2 After a crash at any point, it recovers to a prefix of its barrier epochs, shown by a test that kills writes at random points
- [ ] #3 Cleaning rewrites segments that are mostly freed extents, and space is reclaimed, shown by a test
- [ ] #4 fxdax's Storage over image paths uses it, and fxdax's tests, including cloning and remounting, pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
