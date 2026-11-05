---
id: TASK-173
title: >-
  The collector recovers a child once, by its parent's state, though a record
  found on relisting names another origin
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:38'
updated_date: '2026-11-05 15:39'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 633000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket's TLA+ (TASK-165.2), the model BucketPointsDeep, before its fix was modelled. A pass reads the records before a child has one; recovering its parent S it finds S's fork to child C in S's state, unrecorded, and recovers C as S's fork at that segment (forks_from_states), marking C taken. Meanwhile a restore of point P took the ID C for its own fork (fork_point: C's record names P's origin, S at an earlier segment), so S's fork to C is refused (ChildTaken). The pass, S recovered from a checkpoint of its own, lists the records again (records_relisted) and finds C's record, but recover_live queues only branches not in live.taken: C is never recovered by its record's origin. What S's state at the restore's segment needs (S's segments up to there, which S's checkpoint covers) is deleted, and C recovers from nothing: lost. Trace: S forks P (point) at 1 and a point to C at 2; C restored from P before S records it; P deleted; S checkpoints at 2 dropping P; a pass started before C's record recovers S, C by state, relists, deletes segments 0 and 1. crates/extent_remote/src/collector.rs, recover_live and Live::taken. The model keys recoveries by branch and origin (specs/bucket/Bucket.tla, RecoverAll), and its may_land asks every recovery of a branch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test in collector.rs or the simulation that loses the restore's child with the code as it is, and passes after
- [x] #2 recover_live recovers a branch again when a record names another origin than the one it was recovered by; may_land and what is named go by every recovery
- [ ] #3 The simulation's restores profile makes the case likely (a fork and a restore naming one child), and a seed finds it with the fix reverted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rules::recoveries_by_origins: the collector keys what it recovered by branch and origin (Live::taken), and recovers a branch again by each origin a record or a parent's state names; may_land asks every recovery of a branch (Live::branches holds each).
2. Direct test in collector.rs: a pass lists the records; meanwhile S forks point P, C is restored from P before S's fork to C is recorded (refused), P deleted, S checkpoints; the pass recovers C by S's state, relists, and must recover C again by its record's origin. Red with the rule off: C's recovery and the next pass fail Lost.
3. specs/bucket: rule recoveriesByOrigins, mutation NoRecoveriesByOrigins (BucketPointsDeep's model).
4. Simulation: restores naming a fork's child, if cheap; else noted.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in the collector as specs/bucket models it: Rules::recoveries_by_origins. Live::taken is keyed by branch and origin (Origin is now Ord), recover_live queues and skips by (origin, branch), and Live::branches keeps every recovery's Ended, Pass::may_land asking each (Ended::lands). What is named was already the union of every recovery's extents.
Red/green: collector::tests::children_restored_from_points_keep_their_origins_though_their_ids_were_forked_to (the pass then deletes 2, the next passes) and ..._keeping_their_origins_need_recoveries_by_origins (rule off: the next pass fails Lost("branch 3: what its recovery starts from keeps going")), the trace of the task in a Meanwhile bucket at the pass's first listing of the parent's checkpoints.
Spec: rule recoveriesByOrigins (Recovered(acc, q)), mutation NoRecoveriesByOrigins on BucketPointsDeep's model: violates NothingNeededGone in 39s; BucketPointsDeep holds (2,962,763 states, 76s). The header's TASK-173 difference is gone.
Blind spot: the simulation's IDs are always fresh, so no restore ever named a fork's child, and the collector's tests had one origin per branch. Class test: the simulation's restore() now takes, half the time, the ID of a fork's child not yet recorded, acknowledged or opened (Restore::taken, Unfork::taken give the fork its child back when its record came first); profile restores_taking_forks_ids (more forks, fewer restores) holds 200 seeds; a probe over 400 seeds saw 158 such restores, 54 won by the restore, 9 by the fork, 2 found by a failed restore's deletion. AC #3 not met: test_needs over 0..20,000 of restores and of restores_taking_ids finds no seed needing the rule: the window needs the pass to list the records before the point's record exists and read the parent's checkpoint after the restore, the point's deletion and the checkpoint, and other restores of the point keep its origin. Noted in sim/mod.rs and doc-7.
Look around: the other maps of the collector keyed by branch alone are records (one per key, right) and Pass::branches, now per recovery; branch::delete recovers by the record's origin alone, which is right, the record being the branch. Structure: the bug was a key too narrow; keying recoveries by (branch, origin) makes that class impossible, nothing else needed.

Verified 2026-11-05 at 10331f633 (collector-fixes). Simulation, release: every rule test (21, deletions_need_fences seed 4505, collection_needs_records_kept seed 1) and SIM_SOAK=100 of every holds test (20,000 seeds each, everything 30,000), 43/43 pass in 327 s. scripts/check-tla.sh, all specs and traces: exit 0; every bucket model holds (BucketForks 9,684,100 states 233 s, BucketPointsDeep 149 s), every mutation finds its violation (NoRecoveriesByOrigins 120 s, NoRecordsKept 5 s, NoDeletionsFence 2 s). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2112/2114, the two failures unrelated flakes under a load average of 53, each passing alone (fxdax fork_faults each_write_of_a_failed_forks_cleanup_going_wrong 7/7, filed TASK-184: claim.new shared by two writers; fxfs test_trim 3/3, filed TASK-185); shuttle part run alone, all pass. Linux (Docker, OrbStack, rust:1 arm64): cargo nextest run -p extent_remote -p extent_store 433/433 (572 s). scripts/upstream-changes.py --check: exit 0. Not run: GCE (scripts/test-gce.sh), so DoD #1 stays unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The collector recovers a branch once by each origin it is named by (Rules::recoveries_by_origins): a child it recovered by its parent's state, that fork refused as a restore took the ID, is recovered again by the restore's record found on relisting, and may_land asks every recovery. Direct test red with the rule off (the next pass Lost); specs/bucket rule recoveriesByOrigins and mutation NoRecoveriesByOrigins; the simulation's restores now take pending forks' children's IDs (restores_taking_forks_ids), though no seed reaches the rule's window (AC #3 unmet, as the brief allowed: only if cheap). Verified by the soak, check-tla.sh, macOS and Linux runs; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
