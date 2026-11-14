---
id: TASK-395.16
title: A cell pass makes 7 LISTs and a delimited LIST of its cell
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13 23:50'
updated_date: '2026-11-14 03:12'
labels:
  - collector
  - performance
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 790000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured on ubu1 (run t395sweep, docs/measurements/ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e): every pass over a changed cell makes list 7, list-delimited 1 (70k LISTs a sweep of 10k cells). TASK-395.4's design says a pass lists its cell's prefix once, plus the relisting the spec requires. Find what each LIST is for, and whether one listing of the prefix (paged) can serve them, as LISTs are the costliest request at scale.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each LIST a pass makes is accounted for in doc-7, and those one listing of the prefix can serve are gone
- [x] #2 A request-counting test pins a pass's LISTs per cell
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Account for each LIST of a pass (code: collector::survey/records, recovery::attempt/follow/state_at, referenced, passable, needing, resurrected).
2. Serve from the pass's first listing of the cell (candidates_first): the records' first listing (Bucket.tla GcStart reads records with the candidates) and each branch's first checkpoint listing in recovery (Checkpoints::listed, once a branch; restarts list anew; newest_checkpoint checks after the segments).
3. The records' relisting after the states takes the records read before as known, so it lists once when nothing is new (it listed twice).
4. Keep separate, and say why in doc-7: the records listed after reading them (GcRecords), each branch's newest-checkpoint check, records relisted and references after the states (recordsRelisted, referencesRelisted), references by delimiter.
5. Test: collector::passes_list_their_cell_once_and_what_the_spec_relists pins the LISTs of a pass over cells of one and three branches. No spec change.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), commit 286f4cb43 on branch t3/395-sweep-speed (both tasks in one batch with TASK-395.15).
The 7 LISTs of a pass over a one-branch cell (ubu1 t395sweep): the cell (passed::list); the records twice (Collector::records listed until none new, its first listing finding all new); the branch's checkpoints twice (recovery::attempt, then follow's newest_checkpoint check); the records twice more after the states (the relisting called records() afresh, which took every record for new, so listed again). The delimited one: <C>refs/ (referenced). Rarer ones: passable (no lease, no cell object), state_at of in-cell parents, unless_checkpointed, needing, resurrected.
Changed: the cell's listing serves the first of the records and of each branch's checkpoints (bucket::under, recovery::Checkpoints::listed, served once a branch, a restart lists anew); the relisting takes the records read before as known. Now 4 LISTs and 1 by delimiter; a cell of n branches 3 + n and 1. Kept, and why, in doc-7 (A pass over a cell, The LISTs a pass makes): records listed after their reads (GcRecords), each branch's newest-checkpoint check, records and references after the states, references by delimiter. Not done: one listing of <C> after the states for records and checks (lists every segment and reference again). Spec unchanged: Bucket.tla's GcStart reads the records with the candidates; the collector's recovery is atomic in the model, its code's split justified by newestCheckpoint as before.
Test: collector::passes_list_their_cell_once_and_what_the_spec_relists (one branch: exact LISTs; three: 7 in order, checkpoints any order); red with the snapshot off (6 LISTs, a checkpoint listing twice).
Sim: the per-request changes moved two rule searches' seeds: deletions_need_fences now 120,000..130,000 (122,627; none in 110k-120k, 130k-150k; holds with the rule), deletions_need_records_reread 80,000..90,000 (83,552; none of 20k-69,807; 69,807 fails with every rule on, TASK-403 noted). Searched 6 processes at once.
Verified (macOS): cargo clippy --workspace --tests (nothing new; fxfs-unicode needs UCD_ZIP as test.sh sets); cargo nextest -p extent_remote -p fxdax 639/641 then the 2 fixed/known (my lock test's pinned hash, fixed; TASK-328's flake, 1 of 3 alone, noted there); scripts/test.sh: clippy gate, workspace 2356/2356, sim 61/61 (after re-seeding), Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean. Local LISTs a sweep of 1,000 cells: 4,000 + 1,011 delimited (7,000 + 1,011 before). Not run: ubu1.

ubu1 run t395w6 on 79c8249e1 (workspace 2627/2627, sim 61/61, Firecracker 30/30 and 31/31, mutants 0 broken, guests pass); macOS scripts/test.sh passed; scripts/upstream-changes.py --check passes. On main as 79c8249e1. ubu1 sweep t395sweep2: 40,000 LISTs a first sweep of 10k cells, against 70,000 (docs/measurements/ubu1-gc-sweep-2026-11-14T0233Z-e344b176).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A pass's first listing of its cell also serves as its first listing of records and of each branch's checkpoints, and relisting records after the states costs one LIST: 4 LISTs and a delimited one a one-branch cell (3 + n for n branches), from 7 and 1; doc-7 accounts for each remaining LIST. Pinned by passes_list_their_cell_once_and_what_the_spec_relists; measured on ubu1, 40k LISTs a sweep of 10k cells against 70k. ubu1 run t395w6 on 79c8249e1 (workspace 2627/2627, sim 61/61, Firecracker 30/30 and 31/31, mutants 0 broken, guests pass); macOS scripts/test.sh passed; scripts/upstream-changes.py --check passes. On main as 79c8249e1.
<!-- SECTION:FINAL_SUMMARY:END -->
