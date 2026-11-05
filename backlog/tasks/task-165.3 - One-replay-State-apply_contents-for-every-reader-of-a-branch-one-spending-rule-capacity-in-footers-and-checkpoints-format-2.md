---
id: TASK-165.3
title: >-
  One replay: State::apply_contents for every reader of a branch; one spending
  rule; capacity in footers and checkpoints (format 2)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 05:13'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 625000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 3; TASK-160.15 AC #2. Footers are replayed three times with different rules for spent IDs, next sequence and continuity, which explains TASK-150.6, 79.1 and 79.6. Capacity is in the local record only, so a branch taken elsewhere has whatever capacity its taker gives it, and moving records into the bucket needs it in the protocol.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote has one function applying a segment's contents to a branch's state, used by recovery, the writer, the collector and extent_store's rebuild; no other code applies a footer
- [ ] #2 One spending rule: every free of the branch's own extents in a footer spends it; a test (seal and free within one segment, recover elsewhere) agrees with the live store
- [ ] #3 Capacity is branch state set by commits (Contents, Footer, State, checkpoints); a resize recovers elsewhere; a fork inherits it; a point never changes it
- [ ] #4 layout::FORMAT is 2 and objects of any other format are refused with Error::Format, older ones too
- [ ] #5 The simulation, the crash rounds (which compare rebuild with the live store) and the workspace pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One spending rule (behaviour): State::apply spends every free of the branch's own extents. Batch/Plan footers drop what their commits sealed and later freed, as the store's Window does, so the simulation's footers have the store's shape; the simulation checks each branch's spent against its commits; remote.rs: seal and free within one segment, taken elsewhere, refuses the ID as the live store does. Red before.
2. Refactor: a Footer is its chain's place and its Contents (own extents' locations derived from it): one shape for what a segment's commits made, which the replay applies.
3. Refactor: one replay. recovery::State<P: Places>::apply = continuity + apply_contents; Layers become a Places (Layered); extent_store depends on extent_remote and rebuild replays footers with State::apply over its local Extents; index.rs's footer replay deleted.
4. Capacity (behaviour): Contents::capacity, State::capacity (checkpoints), Batch/Plan commits, a store's commit after a resize carries it, Snapshot::capacity; remote.rs diff states it, take adopts it; simulation commits resize and the check compares; tests: resize recovers elsewhere, fork inherits, point never changes, checkpoint holds it.
5. FORMAT 2 (behaviour): records of any other format refused with Error::Format, older too; test.
6. Verify: macOS workspace+shuttle, sim soak, crash rounds; Linux Docker extent_store/extent_remote/fxdax.
<!-- SECTION:PLAN:END -->
