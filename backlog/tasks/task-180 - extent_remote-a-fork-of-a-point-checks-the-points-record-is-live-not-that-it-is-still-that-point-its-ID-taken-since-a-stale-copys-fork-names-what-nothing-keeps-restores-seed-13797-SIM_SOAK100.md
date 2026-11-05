---
id: TASK-180
title: >-
  extent_remote: a pass fenced by a later one, which read a point live before
  its deletion and its fork being made after, finds what the fork names lost and
  fails rather than stopping (restores seed 13797, SIM_SOAK=100)
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
ordinal: 637000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-175's soak (SIM_SOAK=100 of points_restored_on_many_hosts_at_once, release), seed 13797 on task-175: 'collector: lost: branch 73: what its recovery starts from keeps going'. Pass 9 (collector lease epoch 2) read the records, point 0x17 (Point of branch 2 at 5) live among them. Pass 43 took the lease (epoch 3), read 0x17 deleted, and reclaimed 2's segment 5, which only the point's state needed. Then a host forked the point to 0x73 (fork_point: the record Forking { origin: (2,5), point: 0x17 }, then finish_point_fork, finding the point deleted, failed, and 0x73 was deleted after). Pass 9's records() listing again for new records found 0x73, and kept its earlier read of 0x17, live: so abandon() kept 0x73, and its recovery, state_at(2,5) with segment 5 gone, failed Lost, which stops a pass as an error. The pass was fenced: before deleting it would have stopped quietly; what a pass fenced reads may be older than what the pass that fenced it, deleting since, read. A pass not fenced can't see this: the pass that deleted read after it took its lease. First taken for a point's ID being taken by another branch (Debug prints IDs in decimal); that can't be: a point's record is never dropped. Reproduce: SIM_SEED=13797 SIM_PROFILE=restores cargo nextest run --release -p extent_remote --run-ignored only sim::one (task-175 at 209e379fa).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test pinning it, red before the fix
- [x] #2 A pass that finds what a live branch needs lost stops without failing if a later pass took the collector's lease from it, as it stops before deleting; restores soaks clean at SIM_SOAK=100
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix (87f37bac7): Collector::collect: recover_live failing Lost, a pass that a later one took the collector's lease from (collector::fenced) returns Ok(Report::default()), as it stops before deleting. Rules::passes_fenced_stop. Test: passes_fenced_finding_forks_of_points_lost_stop (a Hooked bucket, at the pass's second listing of the records, deletes the point, runs a later pass that reclaims what it named, and forks the point): Ok, nothing done; with the rule off it fails as the simulation found (Lost("branch 4: what its recovery starts from keeps going")). Seed restores 13797 passes. First suspected a point's ID taken by another branch (the Debug output's IDs are decimal); disproved: a deleted point's record is never dropped (Record::may_drop: roots only), so finish_point_fork's check stands; reverted. Blind spot: the soak's depth, and no test of a pass fenced midway reading what the other pass deleted. Looked around: Pass::new and compact tolerate objects gone (moves read as none, gets skipped); abandon reads only records, which aren't dropped for forks. Structure: a pass not fenced can't see this (what a pass deletes it read after taking the lease), so stopping when fenced is exact. doc-7 (Garbage collection, step 1) says it.

Verified 2026-11-05 at 348d941e8. Simulation soak, release, SIM_SOAK=100 of every holds test (0..20000 each, everything 0..30000): 14/14 pass (it found seeds 210, 3953, 9030, 12424 and 13797 before TASK-177 to 180; main at 13db5c67c fails bases 1775 and stale 3953 at the same soak). Rule tests: every test_needs passes, collection_needs_records_listed_after_states on its own profile now (348d941e8). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2078/2079, the one failure that rule test before 348d941e8, passing alone after; shuttle part (run alone, as the script stops at the workspace's failure) all green. Linux (Docker, OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p extent_store 406/407, the same rule test, passing there after 348d941e8 (176 s). scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed in this batch on task-175 (see notes for the commit, the test red before and the seed); verified with TASK-175's SIM_SOAK=100 soak, macOS workspace and shuttle, Linux in Docker; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
