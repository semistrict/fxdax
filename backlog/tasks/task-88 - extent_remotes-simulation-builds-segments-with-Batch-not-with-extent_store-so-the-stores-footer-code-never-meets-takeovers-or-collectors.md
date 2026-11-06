---
id: TASK-88
title: >-
  extent_remote's simulation builds segments with Batch, not with extent_store,
  so the store's footer contents never meet takeovers or collectors
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
updated_date: '2026-11-06 00:47'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: task
ordinal: 262000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left from the object-storage protocol refactor (integrate-2, review finding 'Segment footers come from two producers'). At dde8d58d3 the producers are still two: the simulation's sessions close segments with batch::Batch (crates/extent_remote/src/batch.rs:126-166, Batch::contents and close_forking; crates/extent_remote/src/sim/world.rs:11, 1090), and a store closes its own (Segments::close, crates/extent_store/src/segments.rs:803-841, from its Window, segments.rs:1214-1299). Since TASK-165.3 both make one shape of footer, a chain place and Contents (a8ca6cb26), under one spending rule (530248363), applied by one replay (f3f6b616f), so they can differ only in what a segment's Contents say. But extent_store's choice of what a window seals, frees and forks, and its next_sequence at a close, is still never run against takeovers or the collector. The review traced a next_sequence/orphan case (TASK-79.3) that the simulation therefore can't reach.

Each session would need an extent_store Store of its own, attached as fxdax's remote.rs does (start, crates/fxdax/src/remote.rs:1132-1222, diffing a snapshot). extent_store has depended on extent_remote since f3f6b616f, so the simulation can't build a Store where it is, in extent_remote, without a cycle: it moves to extent_store, or to a crate above both. TASK-165.9 moves the uploader into extent_store and deletes the attach's diff, which is where to do it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The simulation's writers commit through extent_store's Store, and their segments' footers come from it
- [ ] #2 The simulation reaches TASK-79.3's case: a test seeds it and fails before TASK-79.3's fix
- [ ] #3 The extent_remote simulation tests pass, in a time comparable to now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: description brought to dde8d58d3: TASK-165.3 made one footer shape and one replay, leaving two producers of Contents; extent_store now depends on extent_remote, so the simulation can't build a Store from extent_remote.
<!-- SECTION:NOTES:END -->
