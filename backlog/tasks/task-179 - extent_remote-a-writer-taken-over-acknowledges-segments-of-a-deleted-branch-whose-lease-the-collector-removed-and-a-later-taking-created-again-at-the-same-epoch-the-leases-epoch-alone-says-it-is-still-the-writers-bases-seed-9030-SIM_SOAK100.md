---
id: TASK-179
title: >-
  extent_remote: a writer taken over acknowledges segments of a deleted branch
  whose lease the collector removed and a later taking created again at the same
  epoch: the lease's epoch alone says it is still the writer's (bases seed 9030,
  SIM_SOAK=100)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 08:46'
updated_date: '2026-11-05 09:43'
labels:
  - remote
  - collector
dependencies: []
priority: high
type: bug
ordinal: 636000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-175's soak (SIM_SOAK=100 of bases_deleted_under_live_forks, release), seed 9030 on task-175: 'branch 74 does not open: lost: ... keeps going', and every pass after stopping on 74. Branch c's writer (epoch 1) uploaded segments 5 and 6, forks, ahead of 4. c's deletion ended its lease, recovered c up to 3 (4 missing), found no forks, and marked c deleted; a pass removed c's lease (a deleted branch's) and, c not live, its segments 5 and 6. Then a taking of c (a host opening it) created the lease again, at epoch 1, before finding the record deleted. Segment 4 landed; the writer's acknowledgement read the lease, found epoch 1, its own, and acknowledged 4 to 6, giving the forks' children 74 and 7a records naming c at 5 and 6, gone. Writer::acknowledge and Writer::take check branch::lease_epoch against the writer's epoch; a lease removed and taken again restarts at 1 (take_lease: no lease, epoch 1), so the epoch repeats: the check must be the writer's taking (its holder, unique to each), not its number. Traced with a sequence of the bucket's puts and deletes. Reproduce: SIM_SEED=9030 SIM_PROFILE=bases cargo nextest run --release -p extent_remote --run-ignored only sim::one (task-175 at 209e379fa).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test pinning it, red before the fix
- [x] #2 Writers acknowledge, and finish taking, only while the lease is their taking's (epoch and holder); bases_deleted_under_live_forks soaks clean at SIM_SOAK=100
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (01146d018): branch::holds_lease (lease epoch and holder), which Writer::acknowledge and Writer::take ask instead of comparing lease_epoch; the Writer keeps its holder. Rules::leases_by_takings. Test: writers_acknowledge_nothing_once_their_epoch_is_taken_again (the writer uploads, the branch is deleted, a pass removes the lease, a taking from a record read before the deletion recreates it at epoch 1): Fenced; with the rule off it acknowledges (Ok(Some(1))). Seed bases 9030 passes. Blind spot: the soak's depth, and no test removing a lease and taking it again while a writer of that epoch lives. Looked around: other lease_epoch comparisons: the collector's own (gc/lease is never removed), collector::behind (compares with a checkpoint's epoch; a branch whose lease was removed is deleted, not live), the simulation's check of live writers (compares with the writer's epoch for a live branch's lease, never removed). Writers mint from their epoch's range; a recreated lease's taking fails on the deleted record before minting. Structure: a lease's identity is its taking, which Lease::holder exists to say (ambiguous updates are resolved by it); the epoch number was being used as identity. doc-7 (Fencing) says it.

Verified 2026-11-05 at 348d941e8. Simulation soak, release, SIM_SOAK=100 of every holds test (0..20000 each, everything 0..30000): 14/14 pass (it found seeds 210, 3953, 9030, 12424 and 13797 before TASK-177 to 180; main at 13db5c67c fails bases 1775 and stale 3953 at the same soak). Rule tests: every test_needs passes, collection_needs_records_listed_after_states on its own profile now (348d941e8). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2078/2079, the one failure that rule test before 348d941e8, passing alone after; shuttle part (run alone, as the script stops at the workspace's failure) all green. Linux (Docker, OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p extent_store 406/407, the same rule test, passing there after 348d941e8 (176 s). scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed in this batch on task-175 (see notes for the commit, the test red before and the seed); verified with TASK-175's SIM_SOAK=100 soak, macOS workspace and shuttle, Linux in Docker; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
