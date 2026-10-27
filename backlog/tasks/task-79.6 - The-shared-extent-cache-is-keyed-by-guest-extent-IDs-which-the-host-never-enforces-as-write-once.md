---
id: TASK-79.6
title: >-
  The shared extent cache is keyed by guest extent IDs, which the host never
  enforces as write-once
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-27 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 194000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cache entries keyed by (arena, ExtentId) are shared across branches, and the host lets a branch FREE and then rewrite or writably MAP one of its own IDs. On commit, seal_mapped moves the new pages into another store's entry for the old extent, so a writer guest can inject data into a clone or fork that still holds that extent.

Where: crates/fxdax/src/extents.rs:1690, crates/fxdax/src/extents.rs:1288, crates/fxdax/src/extents.rs:1322, crates/extent_store/src/store.rs:1385.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.4, TASK-26.12, TASK-42.2, TASK-43.2. Not tracked. The extent cache keyed by extent ID (TASK-26.4, TASK-26.12) and by arena (TASK-43.2) both rest on 'IDs are never reused' (TASK-42.2), which the host never enforces for a branch's own IDs.
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store: Spent, the sequence numbers of the branch's own extents freed once sealed, as runs: Store::free records them; begin_write and Store::may_open refuse them (Spent, EPERM in fxdax), as they refuse sealed extents held. Freed-while-open extents, never sealed, may be written again, and so may IDs minted but never written (Fxfs's journal tail, across mounts).
2. Persisted: recovery spends a delta's frees of own extents not written in the same epoch, and a footer rebuild its frees; checkpoints keep the runs as 'spent' rows of their table (local only; object storage's checkpoints don't hold them).
3. fxdax map_open checks may_open before a writable MAP opens a new extent.
4. Tests: extent_store freed_extents_are_written_no_more (across two reopens, deltas then checkpoint; an unwritten ID below next_sequence still writable), layout's checkpoint table round trip with spent rows, fxdax extents::freed_extents_are_written_no_more, fxdax filesystem images_opened_again_take_synced_writes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
First version (spend everything below next_sequence at open) broke every guest mounting a store written before (r149: the journal writes the tail it minted ahead in an earlier mount): replaced by the persisted record above; images_opened_again_take_synced_writes fails under the first version, passes now.
<!-- SECTION:NOTES:END -->
