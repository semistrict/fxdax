---
id: TASK-178
title: >-
  extent_remote: a deletion that found the branch live fails Lost when another
  deletion finishes meanwhile and the collector reclaims what recovering the
  branch needs (stale_placements seed 3953, SIM_SOAK=100; main too)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 08:36'
updated_date: '2026-11-05 09:43'
labels:
  - remote
  - collector
dependencies: []
priority: high
type: bug
ordinal: 635000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-175's soak (SIM_SOAK=100 of stale_placements_take_nothing_back, release), seed 3953, on task-175 and on main (13db5c67c) alike: "branch 1d's stale delete: lost: branch 1d: what its recovery starts from keeps going". A placement cut off deletes branch 1d, a fork of 2: it finds the lease final (the latest placement's deletion began since), reads the record live, and recovers the branch to give its forks' children their records. Meanwhile that other deletion finishes and a pass of the collector reclaims 1d's checkpoints and segments and the parent's segments up to the fork, which only 1d needed: the recovery, finding no checkpoint of 1d's and the parent's segments gone, starts again until it gives up (Error::Lost). branch::delete takes a record deleted when it first reads it as done (TASK-79.78), but not one deleted while it recovers the branch; the collector does (recover_live: deleted since its record was read, it is not live). Instrumented: 63 restarts at recovery::state_at's 'acknowledged segments go only once a checkpoint covers them'. Reproduce: SIM_SEED=3953 SIM_PROFILE=stale cargo nextest run --release -p extent_remote --run-ignored only sim::one.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test pinning the race, red before the fix
- [x] #2 A deletion whose recovery of the branch fails Lost, the branch's record now deleted or gone, is done: the deletion that marked it gave its children their records; stale_placements soaks clean at SIM_SOAK=100
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (209e379fa): branch::delete, its recovery of the branch failing Lost, reads the record again: deleted or gone, the deletion is done (Ok), as recover_live takes a branch deleted since its record was read. Test: deletions_finished_by_others_meanwhile_are_done (a Meanwhile bucket runs the other deletion and a pass as the first lists the fork's checkpoints): red before with Lost("branch 2: what its recovery starts from keeps going"). Seed stale 3953 passes; on main it fails the same way. Blind spot: soak depth (seed 3953 is past SIM_SOAK=10's 2000), and no test of two deletions of one branch racing a pass. Looked around: the other recoveries of a branch that may be deleted meanwhile: recover_live (handles it), Writer::take (checks the record after taking the lease, Error::Deleted), delete_fork (goes through delete). Structure: the deletion's own read of the record had the rule; the recovery after it now does too. doc-7 (Forks, Deleting a branch) says it.

Verified 2026-11-05 at 348d941e8. Simulation soak, release, SIM_SOAK=100 of every holds test (0..20000 each, everything 0..30000): 14/14 pass (it found seeds 210, 3953, 9030, 12424 and 13797 before TASK-177 to 180; main at 13db5c67c fails bases 1775 and stale 3953 at the same soak). Rule tests: every test_needs passes, collection_needs_records_listed_after_states on its own profile now (348d941e8). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2078/2079, the one failure that rule test before 348d941e8, passing alone after; shuttle part (run alone, as the script stops at the workspace's failure) all green. Linux (Docker, OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p extent_store 406/407, the same rule test, passing there after 348d941e8 (176 s). scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed in this batch on task-175 (see notes for the commit, the test red before and the seed); verified with TASK-175's SIM_SOAK=100 soak, macOS workspace and shuttle, Linux in Docker; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
