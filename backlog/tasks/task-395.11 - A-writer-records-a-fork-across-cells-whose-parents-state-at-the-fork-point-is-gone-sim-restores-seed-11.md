---
id: TASK-395.11
title: >-
  A writer records a fork across cells whose parent's state at the fork point is
  gone (sim restores seed 11)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13 15:51'
updated_date: '2026-11-13 19:59'
labels:
  - bug
  - bucket
  - collector
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 785000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the simulation's points_restored_on_many_hosts_at_once, seed 11 (restores profile), on the TASK-395.4 review fixes: branch 1/2/2's uploader acknowledged and recorded a fork at its segment 3 to point 1/23/23, which had no record, after the parent's segments 1 to 5 were gone (its checkpoints 0xb and later covering them), so the parent's state at the fork point could not be recovered: recovery::checkpoint_at, which recording a fork across cells now calls, failed Lost and failed the uploader. Before TASK-395.4's review the record was created all the same, naming a state no reader could recover (the sim passed: the child was no host's, deleted after). No pass kept the fork point: the bucket's checkpoints of the parent did not name the fork, only the uploader's state did, so how the uploader's state came to hold a fork the branch's checkpoints don't is the question (a stale state, a WAL rebased, a fork segment of another history). Since the review, record_fork records such a child deleted and warns (branch::record_fork). Reproduce: SIM_PROFILE=restores SIM_SEED=11 cargo nextest run --cargo-profile sim -p extent_store --run-ignored only -E 'test(=sim::one)', and look for the warning.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is found and fixed or shown benign; the deleted-record path either goes or is tested
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce (seed 11 moved; search restores seeds with a temporary panic on the Lost path).
2. Trace the interleaving, find why the state at the fork point is gone.
3. Fix at the cause; test the real interleaving (Hooked bucket) instead of a fabricated state.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round (@claude, 2026-11-13; 218be7c5a). Seed 11 no longer reached the path (seeds moved); a temporary panic on record_fork's Lost arm and a search of restores found seed 2545. Cause: a stale read, not a stale state. The writer acknowledged a sound fork; record_fork read the child's record (none), and before checkpoint_at the child was deleted by its host (branch::delete of a child with no record: a deleted Kind::Own), the parent's checkpoint dropped the fork (child deleted), and a pass freed the fork point; checkpoint_at then failed Lost. The structure does not let a writer acknowledge a fork whose state is gone: the state at a fork point goes only once the child is deleted (the parent's state names the fork until a checkpoint drops it, which a deleted child alone lets; the parent's deletion records the child before marking the parent deleted). Fix: record_fork reads the child's record again after the checkpoint (narrowing TASK-395.10's stale-recorder window too) and goes by it; on Lost with none there it fails Lost (would break the argument). The deleted-record path is gone. Test forks_recorded_after_their_children_went_record_nothing reproduces the real interleaving (Hooked after-get hook: child deleted, next writer checkpoints, sweep) instead of the old fabricated state; failed_forks_across_cells_of_a_state_gone_are_deleted_from_the_first keeps delete_fork's case. doc-7 and OPEN_QUESTIONS.md updated.
<!-- SECTION:NOTES:END -->
