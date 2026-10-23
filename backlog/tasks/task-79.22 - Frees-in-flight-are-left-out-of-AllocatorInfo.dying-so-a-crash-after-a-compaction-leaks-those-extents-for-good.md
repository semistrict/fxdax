---
id: TASK-79.22
title: >-
  Frees in flight are left out of AllocatorInfo.dying, so a crash after a
  compaction leaks those extents for good
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
free_dead took its batch out of the extent table's dead queue while the device freed it. A compaction in that window wrote AllocatorInfo.dying, and its filter of deleted owners, without those extents, so a crash leaked them for good. The extent table now keeps the batch being freed, which dying() and holds_dead_of() count, and FakeDevice can hold frees in flight for the test (1001f265). Before the fix, test_compaction_during_frees_keeps_their_extents_dying failed with 'extent leaked' and extents_being_freed_are_dying with an empty dying(). Both pass on macOS and on Linux (GCE), and upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
