---
id: TASK-79.22
title: >-
  Frees in flight are left out of AllocatorInfo.dying, so a crash after a
  compaction leaks those extents for good
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 210000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
free_dead takes its batch out of dead before the FREE_MANY round trips, and pushes it to unsealed only afterwards. A compaction in that window persists a dying list without those extents, so if a crash undoes the frees, they are absent from the tree and from dying and leak permanently. freed_owners has the same gap for deleted volumes.

Where: crates/fxfs/src/object_store/allocator.rs:2036, crates/fxfs/src/object_store/allocator.rs:980, crates/fxfs/src/object_store/allocator.rs:2553, crates/fxfs/src/object_store/allocator.rs:2572, crates/fxfs/src/object_store/journal.rs:2204.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.3, TASK-41. Not tracked. TASK-26.3 added AllocatorInfo.dying for this class of compaction leak. TASK-41's batch 1 added FREE_MANY, whose round trips open the window free_dead leaves out of dying.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
