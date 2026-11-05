---
id: TASK-165.3
title: >-
  One replay: State::apply_contents for every reader of a branch; one spending
  rule; capacity in footers and checkpoints (format 2)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 07:47'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design. One replay: extent_remote::recovery::State<P: Places> (P = Extents, the bucket's, by default). State::apply(footer) checks continuity (doc-7, Fencing) and calls State::apply_contents(seq, epoch, &Contents), which seals (Seal::Run / Seal::Own, each at its bucket Location), frees, spends, sets root, last commit, next sequence (largest named), forks and capacity. Readers place extents as they keep them: Extents (recovery, writers, points, deletion), Layered (the collector's layers, which shadow inherited extents as the replay seals or frees them; Layers::apply is gone), and extent_store's index::Replayed (rebuild, its extents placed in the store's segments). extent_store depends on extent_remote. index.rs's own footer replay is deleted; rebuild anchors on its checkpoint's segment as before, then State::apply. Footers are applied nowhere else (delta replay reads deltas, which go with the index in 165.9).
A Footer is its chain place (branch, seq, epoch, after) and its segment's Contents. Own extents are named by sequence, and Footer::own / Contents::own_at derive their keys, so a footer can't name its own extents elsewhere. Chain::admit checks only the place; the three admit cases and the writer case that refused unrepresentable footers went with them, and the writer now refuses an own extent uploaded at another key.
Spending: every free of the branch's own extents in a footer spends it (remote recovery's held-only rule is gone). The divergence: a store footer drops an extent sealed by one commit and freed by a later commit in the same segment, keeping only the free, so recovery elsewhere left the ID writable. Blind spot: the simulation's batches kept such extents among their seals, and nothing compared spent. batch::Plan now builds the store's footer shape (sealed then freed = a free alone; sealed and freed by one commit = neither), and the simulation checks each branch's spent against its commits.
Capacity: Contents::capacity (the last a segment's commits set) -> State::capacity, held in checkpoints, copied to a child at the fork; a point keeps it. Store::set_capacity keeps the local record and has the next commit carry it (Uncommitted::resize -> Window::capacity). It does not commit itself, because that would seal extents a guest has open. remote.rs: take() adopts the branch's capacity; an uploader whose branch state says none or another calls Store::restate_capacity, so the next commit carries it. No extra segment at attach: fork tests name segments by number (TASK-157's area). The simulation's commits resize (every fifth tag, so the RNG and seeds are unchanged) and the check compares capacity.
FORMAT = 2; Record::decode refuses any other format (Error::Format), older too.
Tests red before: remote.rs extents_sealed_and_freed_in_one_segment_are_spent_where_their_branch_is_taken (the taken store wrote the ID); sim forks_and_frees seed 0 with the old rule (branch 2 spent ..4, not ..5); resizes_hold_where_their_branch_is_taken without take adopting it (1 GiB, not 1.5 GiB); sim::everything with the replay ignoring capacity; records_of_other_formats_are_refused (format 1 decoded). New: batches_free_what_a_commit_before_held, footers_name_their_own_extents_at_their_writers_keys, capacities_are_set_by_commits, resizes_are_their_next_commits.
Structure: the class (three replays, three rules) is now impossible, because a footer has one applier. Overlap: remote.rs (diff/take/restate, 3 tests) and extent_store segments/state/store; nothing in extentd, specs or guest.

Verified 2026-11-05. macOS: FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2067/2067, Shuttle all parts green; clippy part green; upstream-changes.py --check green. The crash rounds (extent_store tests/crash, which compare rebuild with the live store) pass in the workspace run. Linux (Docker arm64, rust:1, nextest -p extent_store -p extent_remote -p fxdax): 844/852. The 8 failures are environment-only (eu-stack missing x3, grants ENOENT x3, build stamp) plus TASK-157's guests_forks_failing_at_each_step_harm_nothing, which passes alone (36 s). The simulation's rule tests stop at the same seeds as on main (records_listed: 5310, newest_checkpoints: 11464), so their 4-8 min runtimes predate this. Soak SIM_SOAK=10 of every holds test: one violation, stale_creates_landing_past_deletions seed 677 (the only one in 0..20000; main passes all 20000). It is a collector hole that this branch's changed trajectories reach: a fenced writer's late checkpoint names moved locations, so the collector deletes a moves hop the live writer still needs. Filed as TASK-175 with the trace; not fixed here.
<!-- SECTION:NOTES:END -->
