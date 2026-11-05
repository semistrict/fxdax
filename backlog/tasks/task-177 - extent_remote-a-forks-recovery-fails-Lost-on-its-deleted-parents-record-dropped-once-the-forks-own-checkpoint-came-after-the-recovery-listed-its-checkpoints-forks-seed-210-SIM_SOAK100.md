---
id: TASK-177
title: >-
  extent_remote: a fork's recovery fails Lost on its deleted parent's record,
  dropped once the fork's own checkpoint came after the recovery listed its
  checkpoints (forks seed 210, SIM_SOAK=100)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 08:28'
updated_date: '2026-11-05 09:43'
labels:
  - remote
  - collector
dependencies: []
priority: high
type: bug
ordinal: 634000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-175's soak (SIM_SOAK=100 of forks_and_frees, release): seed 210, 'deleting branch 1a: lost: branches/2'. Branch 1a, a fork of 2 at its segment 2, its parent deleted, had no checkpoint of its own when the deletion's recovery of it listed its checkpoints; its writer's checkpoint 0 landed after, and a pass of the collector, recovering 1a from that checkpoint, no longer read 2's record, a deleted branch's, and dropped it. The recovery went on to the parent's state at the fork, which, 2 having no checkpoint up to there, starts from 2's record: gone, and recovery::state_at fails Error::Lost rather than starting again, as it does for a segment or checkpoint gone (doc-7, Checkpoints: a reader that finds a segment missing lists the checkpoints again). Instrumented: 'recovering 1a: branches/2 lost; its checkpoints now [checkpoints/1a/0]'. The same race one level up: a parent's own checkpoint up to the fork landing late makes its parent's record unneeded. It shows with TASK-175's rule on only because its lease reads move the trajectory; with ways_from_seals off seed 210 passes. Reproduce: SIM_SEED=210 SIM_PROFILE=forks cargo nextest run --release -p extent_remote --run-ignored only sim::one (on task-175 at 9ca6a6332).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test pinning the race, red before the fix
- [x] #2 A recovery that started from no checkpoint of a branch up to a segment, finding what it went to instead gone, starts again if one has come since; the simulation soaks clean (SIM_SOAK=100 of every holds test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (379abc75d): recovery::unless_checkpointed. attempt (for a fork's own branch, at any segment) and state_at (for each parent up the chain, up to the fork's segment), finding what they went to instead of a checkpoint lost, list the branch's checkpoints again and restart if one has come; a record gone that no checkpoint explains stays Lost. Under Rules::newest_checkpoint, which already says readers check for a newer checkpoint when a segment is missing. Test: forks_recover_past_parents_records_their_own_checkpoints_let_go (a Hooked bucket runs the fork's checkpoint and a pass as the recovery gets the parent's record): red before with Lost("branches/1"); with newest_checkpoint off, the same. Seeds forks 210 and crashes 12424 pass. Blind spot: the soak's depth: 200 seeds per test and SIM_SOAK=10 (2000) missed seed 210; and no test combined a fork's first checkpoint landing between a recovery's two reads. Looked around: every Restart/Lost site in recovery.rs: segments and checkpoints gone already restart; a record was the one that didn't. Structure: the rule is one ('a reader finding what it started from gone checks for a newer checkpoint'), now applied at every level of the inherited chain; nothing to restructure. doc-7 (Checkpoints) says it.

Verified 2026-11-05 at 348d941e8. Simulation soak, release, SIM_SOAK=100 of every holds test (0..20000 each, everything 0..30000): 14/14 pass (it found seeds 210, 3953, 9030, 12424 and 13797 before TASK-177 to 180; main at 13db5c67c fails bases 1775 and stale 3953 at the same soak). Rule tests: every test_needs passes, collection_needs_records_listed_after_states on its own profile now (348d941e8). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2078/2079, the one failure that rule test before 348d941e8, passing alone after; shuttle part (run alone, as the script stops at the workspace's failure) all green. Linux (Docker, OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p extent_store 406/407, the same rule test, passing there after 348d941e8 (176 s). scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed in this batch on task-175 (see notes for the commit, the test red before and the seed); verified with TASK-175's SIM_SOAK=100 soak, macOS workspace and shuttle, Linux in Docker; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
