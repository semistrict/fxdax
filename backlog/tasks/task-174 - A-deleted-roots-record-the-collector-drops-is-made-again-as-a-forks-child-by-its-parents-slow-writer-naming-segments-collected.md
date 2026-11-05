---
id: TASK-174
title: >-
  A deleted root's record the collector drops is made again, as a fork's child,
  by its parent's slow writer, naming segments collected
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:38'
updated_date: '2026-11-05 15:39'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 634000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket's TLA+ (TASK-165.2), the model BucketForks, before its fix was modelled. Branch S's writer forks child C (an ID a guest chose); another branch takes C's ID (branch::create) and is deleted (its record deleted, kind Root). S's writer acknowledges the fork (its lease read before S's deletion ends the lease) but is slow to record C. S's deletion recovers S, finds the fork, record_fork finds C's record deleted and stops; S is marked deleted. A pass drops C's record (Record::may_drop: a deleted root's) and S's lease and segments, nothing live needing them. S's slow writer's record_forks then creates C's record (Kind::Fork, origin S at the fork): a live branch whose recovery reads segments the pass deleted. Every pass stops on it (Error::Lost) for good. doc-7 keeps deleted forks' records for this reason (a parent's writer slow to record a fork could make the record again); a deleted root's record, whose ID a fork named, is the same case. The model keeps every record (specs/bucket/Bucket.tla, GcFinish's doomedKey). Fixes possible: keep deleted roots' records too (their number is bounded by the branches made, as forks' is), or drop one only once no writer that could name it as a child can still record. crates/extent_remote/src/record.rs, Record::may_drop; collector.rs, doomed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test that makes C's record again after the pass dropped it with the code as it is, and finds no record made again after the fix
- [x] #2 The rule the fix adds is in extent_remote's Rules, with its simulation test and specs/bucket's mutation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rules::records_kept: the collector drops no record, a deleted root's neither (Record::may_drop goes); a parent's slow writer then finds the deleted record and makes none.
2. Direct test: S forks C, a root takes C's ID and is deleted, S's writer acknowledges but records late, S deleted, a pass runs, then S's writer records: no record made again; red with the rule off (C's record made live, the next pass Lost).
3. specs/bucket: rule recordsKept, mutation NoRecordsKept (BucketForks).
4. Simulation, if cheap.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed as the spec models it: Rules::records_kept, Record::may_drop false (a deleted root's went), the collector's doomed and the simulation's garbage oracle going by it. Chose keeping every record over dropping a deleted root's once no slow writer can record: the latter needs a bound on how long a writer stalls, which doc-7 already declined for forks' records.
Red/green: collector::tests::deleted_roots_keep_their_records_from_slow_writers_forking_to_their_ids (the child's record stays the deleted root's; the next pass deletes nothing, Ok) and deleted_roots_keeping_their_records_need_records_kept (rule off: the slow writer makes Fork(1 at 1) of 0x22 and the next pass fails Lost(branches/1)). A Gated test bucket holds the writer's put of the child's record until the parent's deletion and a pass are done.
Spec: rule recordsKept (GcFinish's doomedKey drops a deleted root's record not touched without it, rec' follows), mutation NoRecordsKept on BucketForks's model: violates NothingNeededGone in 2s. The header's TASK-174 difference is gone.
Simulation: a new action, usurp (a root takes a pending fork's child's ID and is deleted, World::usurp, Event::Usurped giving the fork its child back when the fork's record came first); profile usurped holds 200 seeds (forks_ids_taken_by_roots_deleted); collection_needs_records_kept finds seed 1 with the rule off ('branch 32's record says Some(Origin { parent: 2, at: 13 })': the deleted root's record remade as the fork's child).
Blind spot: the simulation's roots never took an ID a fork had named (contest only raced a fork whose segment never went), and no test had a writer pause between its lease read and its records. Class: usurp and the Gated bucket. Look around: TASK-177's restart on a parent's record found gone (recovery::unless_checkpointed) is reached now only if a record goes; its two tests run with records_kept off, so it stays covered. Other deletes of records: none; abandon_claim and abandon mark deleted, never remove. Structure: the record is the one owner of whether an ID is taken; never removing it makes the slow-writer race impossible for every kind, which is the fix.

Verified 2026-11-05 at 10331f633 (collector-fixes). Simulation, release: every rule test (21, deletions_need_fences seed 4505, collection_needs_records_kept seed 1) and SIM_SOAK=100 of every holds test (20,000 seeds each, everything 30,000), 43/43 pass in 327 s. scripts/check-tla.sh, all specs and traces: exit 0; every bucket model holds (BucketForks 9,684,100 states 233 s, BucketPointsDeep 149 s), every mutation finds its violation (NoRecoveriesByOrigins 120 s, NoRecordsKept 5 s, NoDeletionsFence 2 s). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2112/2114, the two failures unrelated flakes under a load average of 53, each passing alone (fxdax fork_faults each_write_of_a_failed_forks_cleanup_going_wrong 7/7, filed TASK-184: claim.new shared by two writers; fxfs test_trim 3/3, filed TASK-185); shuttle part run alone, all pass. Linux (Docker, OrbStack, rust:1 arm64): cargo nextest run -p extent_remote -p extent_store 433/433 (572 s). scripts/upstream-changes.py --check: exit 0. Not run: GCE (scripts/test-gce.sh), so DoD #1 stays unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The collector keeps every record, a deleted root's too (Rules::records_kept, Record::may_drop false), so a parent's slow writer finds the root's deleted record and makes none naming what a pass reclaimed. Direct test red with the rule off (the record made as the fork's child, the next pass Lost); specs/bucket rule recordsKept and mutation NoRecordsKept; simulation action usurp, profile forks_ids_taken_by_roots_deleted, rule test collection_needs_records_kept (seed 1). Verified by the soak, check-tla.sh, macOS and Linux runs; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
