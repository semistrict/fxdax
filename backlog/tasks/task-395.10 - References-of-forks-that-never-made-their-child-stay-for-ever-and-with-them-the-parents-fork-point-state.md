---
id: TASK-395.10
title: >-
  References of forks that never made their child stay for ever, and with them
  the parent's fork-point state
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13 13:47'
updated_date: '2026-11-13 21:55'
labels:
  - bucket
  - collector
  - bug
dependencies: []
parent_task_id: TASK-395
priority: low
ordinal: 783000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxdax's restore_faults (TASK-395.3's crash rounds), and TASK-395.4's review: a reference whose child never gets a record, or gets another's, is never retired, as the child's cell's pass retires only the references its own deleted records' origins name (doc-7, References). The parent's cell then keeps the parent's whole state at that fork point for ever: space, never data. Cases: (1) a clone's host dies after the clone wrote its reference and before its forking record (failpoint point::referred, or its first write lost with its host): its directory is unmade, and deleting it touches nothing in the bucket, as nothing on the host says which child or origin it was; (2) a writable fork's child ID taken by another branch between record_fork's read of the child's record and its create (failpoint branch::referred): object IDs rule this out in production, but not a guest naming any ID; (3) a clone to an ID that already has a record (branch::fork_point's create refused, Deleted or IdTaken): since the review the clone takes its reference back unless the record found names the same origin, so only its host dying between the refusal and the delete leaves it. specs/bucket accepts these (RefPoints: 'a reference left by a fork that failed'). The tests excuse exactly what such a reference keeps (extent_remote::testing::fork_point_keeps), naming this task: crates/fxdax/src/remote/restore_faults.rs and fork_faults.rs, their 'left in the bucket' checks. Ways: the fork's directory recording the child and origin before the reference, so that deleting it unmade gives the child a deleted forking record naming the origin (which its cell's pass retires by); or the parent's cell's pass reading, a grace period after, the records of children named under fork points that only references keep (a GET per such child, rare).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A clone whose host dies between its reference and its record leaves nothing once its directory is deleted and passes ran
- [x] #2 fork_faults and restore_faults no longer excuse references left behind
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rule clonesRecordFirst: a clone creates its forking record before its reference (still before the point's read), so every clone reference has a record naming its origin, retired by its child's cell's pass whatever comes of the clone; a refused clone writes none.
2. Rule referencesTakenBack: whoever finds a fork's child's record naming another origin or none (record_fork's read and refused create; next writer; parent's deletion) deletes the reference.
3. Spec: both rules, mutations NoClonesRecordFirst/NoReferencesTakenBack, invariant ReferencesNamed (named by its child's record or pending), models BucketRefsTaken/BucketRefsClones; check-tla.sh under 5 min a run.
4. Code: Rules fields, branch::fork_point/record_fork/take_back; testing::uncollected flags unnamed references (class oracle); sim Profile::forget (hosts forgetting crash-stopped restores) + leak-needs test; unit tests; remove the fault tests' excuses.
5. doc-7, OPEN_QUESTIONS.md.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round (@claude, 2026-11-13; branch t3/395-references, 865d035af spec, 218be7c5a code and tests, cb10ab0d0 docs).
Retirement rule: no grace period; a reference is always named by its child's record or taken back. (1) clones_record_first: a clone creates its forking record (naming the origin) before its reference, then reads the point: a host dying anywhere leaves a record the collector abandons once the point goes, and the child's cell's pass retires the reference by its origin; a refused clone writes none (the old take-back on refusal is gone with the rule on). (2) references_taken_back: whoever finds the child's record naming another origin or none deletes the reference (record_fork's read and refused create; the next writer and the parent's deletion record every fork again before a checkpoint drops it). Safe because records are kept and their origins never change. Grace-by-age rejected: a pass lists fork points by delimiter, not references, so it would list every reference (a million for a base cloned a million times) and GET each child's record every pass (OPEN_QUESTIONS.md).
Proof: specs/bucket invariant ReferencesNamed (named by the child's record, or pending); NoClonesRecordFirst and NoReferencesTakenBack violate it (1 s each); new models BucketRefsTaken (contest, crash, delete; 1.36M states) and BucketRefsClones (clones whose hosts die; 665k) hold; check-tla.sh: Bucket, Cells, CellsPoints, Compaction, Deletions, Inherit, Own, Placements, Points, PointsDeep, Spans, SpansCollect, Twins, Wal hold, all 52 mutations fail as expected. Not run (14 min-2 h): BucketForks, BucketOverlap, BucketStrays, BucketTakeovers. TLC found one modelling slip of mine on the way (the deletion took back by records before recording; two forks to one child ID), fixed.
Blind spots: the sim's hosts always knew their restores' children (production's unmade directory doesn't), and testing::uncollected kept any reference. Class tests: Profile::forget + profile forgotten (test_holds restores_forgotten_leave_nothing_behind, test_leaks forgotten_restores_need_clones_record_first); uncollected flags a reference whose child has no record and that no live fork pends. Looking around with that oracle found the stale recorder: a writer reads no record, the child is then deleted with none (Own) and another recorder takes back, the writer writes the reference and stops before its create (sim restores 17 and four other profiles). Narrowed (record read again after the checkpoint), not closed: no finite set of earlier lookers can; filed TASK-395.13 (marker in the child's cell, or writable forks recorded pending first), which uncollected excuses by name (child's record naming another origin or none).
Structure: the leak class came from a write whose only retirer keys on a record that may never name it; making the clone's record precede its reference removes the class for clones; writable forks need the same shape (TASK-395.13).
Tests: unit tests in collector.rs (see commit), fault tests unexcused (restore_faults, fork_faults all pass). macOS scripts/test.sh: clippy clean, workspace 2333/2333, sim 61/61, Shuttle all, scripts 312 OK, mutants 0 broken (forks-take-branches-claimed re-anchored, killer kills it), go ok; Linux check (zig) clean. Not run: ubu1.
Also filed TASK-403 (deleted_forking seed 100732 fails on the base too: a log found diverged).

Merged with TASK-395.5/.6/.8 on t3/395-integrated (cherry-picked unchanged; conflicts only in collector tests and docs). Verified on t3/395-integrated (macOS): cargo clippy --workspace --tests (nothing new); cargo nextest -p extent_remote 296/296 (with the 2 new tests); scripts/test.sh: clippy gate, workspace 2352/2352, sim 61/61 (deletions_need_fences re-seeded, see TASK-403), Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean; TLA+ specs/bucket models but BucketForks/Overlap/Strays/Takeovers hold (16) and all 72 mutations fail as expected. Flake TASK-404 filed (commits_reopen_from_their_logs under load, unrelated). Not run: ubu1.
<!-- SECTION:NOTES:END -->
