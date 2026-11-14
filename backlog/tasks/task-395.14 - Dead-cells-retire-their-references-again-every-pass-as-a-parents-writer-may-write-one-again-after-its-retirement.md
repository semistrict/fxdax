---
id: TASK-395.14
title: >-
  Dead cells retire their references again every pass, as a parent's writer may
  write one again after its retirement
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13 18:38'
updated_date: '2026-11-14 15:30'
labels:
  - bucket
  - collector
dependencies: []
parent_task_id: TASK-395
priority: low
ordinal: 787000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the simulation (seed 53 of restores) once TASK-395.6 skipped unchanged cells: branch::record_fork reads a cross-cell child's record live, the child is then deleted and its dead cell's pass retires the child's reference, and the writer puts the reference again (record_fork re-puts it for a child already recorded). Nothing of the dead cell's changes, so a pass that only skipped it would never retire it again, and the reference would keep the parent's state at the fork point (its checkpoint and what it names) for good. TASK-395.6 therefore has every pass over a dead cell, skipping or not, send its retirements again (recorded in the cell's collector lease): a DELETE more per pass for every dead cell with children of other cells, for ever, as before TASK-395.6. Cells are never collected, so dead cells accumulate. Making a reference impossible to write again after its retirement (or detectable by the parent's pass without a GET per reference) would let dead cells cost a pass only its lease and listing. Any change to who writes references must go through specs/bucket (Bucket.tla) first.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A pass over a dead cell unchanged since its last pass makes no request beyond its lease and listing
- [x] #2 A reference written again after its retirement by a slow writer of the parent is retired or never kept: the simulation's restores profile finds no garbage left, and a direct test reproduces seed 53's order
- [x] #3 Bucket.tla models the change, with a mutation TLC fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Done together with TASK-395.13 (see its plan).
1. Model: a recorder finding the child's record live, its own, writes no reference; retirements marked (rule retirementsMarked, mutation NoRetirementsMarked).
2. Direct test of seed 53's order (recorders_finding_their_childs_record_write_no_reference) and of a None-read stale write after retirement (stale_recorders_references_*).
3. Code: record_fork's skip; Passed::retired and Collector::skip's re-send dropped.
4. Tests updated: a skip of a dead cell asks its lease and listing alone.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Integration (@claude, 2026-11-13, branch t3/395-integrated): checked whether TASK-395.10's second read of the child's record in branch::record_fork rules the re-creation out. It does not: the second read is made only when the first found no record. A writer whose first read finds the child's record live and its own (Some, is_child_of) goes on to refer_fork, writing the reference again (rule references_first), then its create stops on the record it read (Start::Found, no read again) and returns Ok. Between that read and that write the child may be deleted and its dead cell's pass retire the reference: the write lands after, seed 53's order, unchanged. So the per-pass re-retirement of a dead cell's references stays (collector::Collector::skip retires passed.retired again; tested by collector::tests::passes_skip_only_what_the_cell_alone_decides, red without it). Way out, for this task: a record found live naming the fork's origin had its reference written by its maker before its create (references_first; a clone's record precedes its reference, but a writer never records a clone's child), so record_fork may skip the write in that case, which removes the re-creation; it changes Bucket.tla's recording (the model re-puts), so the model, a mutation (re-put kept) and seed 53's order as a direct test first, then the code, then drop Passed::retired's re-send.

Verified on t3/395-integrated (macOS): cargo clippy --workspace --tests (nothing new); cargo nextest -p extent_remote 296/296 (with the 2 new tests); scripts/test.sh: clippy gate, workspace 2352/2352, sim 61/61 (deletions_need_fences re-seeded, see TASK-403), Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean; TLA+ specs/bucket models but BucketForks/Overlap/Strays/Takeovers hold (16) and all 72 mutations fail as expected. Flake TASK-404 filed (commits_reopen_from_their_logs under load, unrelated). Not run: ubu1.

Fix: record_fork writes no reference when it reads the child's record live, its own (its maker wrote it first); Passed::retired and Collector::skip's re-send are gone, so an unchanged dead cell costs a pass its lease and listing (passes_skip_only_what_the_cell_alone_decides asserts asked == lease GET/PUT + LIST). The skip alone was not enough: a recorder that read no record, the fork then recorded by another, the child deleted and retired, still writes after; so retirements are marked (TASK-395.13's fix) and the parent's cell's pass deletes it (stale_recorders_references_go_by_their_marks, not-taken order). Seed 53's order: recorders_finding_their_childs_record_write_no_reference, red with the skip removed. The simulation's restores profile finds no garbage (63/63; no seed lost its search, none re-seeded).

Verified on main 111633d6f (pre-rebase 57f568458): macOS cargo clippy gate clean; cargo nextest -p extent_remote 303/303; scripts/test.sh: workspace 2359/2360 (the one failure TASK-328's flake, 4/4 alone after), sim 63/63, Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean; ubu1 run t39513a under fxjob: workspace 2632/2632, sim 63/63, guest tests and e2e pass; upstream-changes.py --check ok; after rebasing onto e5c49d2e3: clippy gate clean, extent_remote+fxdax remote/peers/kept/collect 396/396. TLA+: all bucket models but BucketForks/Overlap/Strays/Takeovers hold (16, with BucketRefsTaken), all 53 mutations fail as expected (NoRetirementsMarked 1-2 s). BucketDeletions grows 3.44M -> 4.64M states (+35%; ~210 s idle from 158 s), BucketCells 2.9M -> 3.68M: every default model stays under 10 min. Not run: the four long models (the decomposition delegate's).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Dead cells' passes that skip make no request beyond lease and listing: Passed::retired's re-send is gone; record_fork writes no reference for a child it reads live and its own (seed 53's order, direct test); a reference re-written after retirement by a recorder that read none goes by its mark with the parent's cell's pass. Modelled in Bucket.tla with mutation NoRetirementsMarked. Verified macOS, ubu1 t39513a, TLC.
<!-- SECTION:FINAL_SUMMARY:END -->
