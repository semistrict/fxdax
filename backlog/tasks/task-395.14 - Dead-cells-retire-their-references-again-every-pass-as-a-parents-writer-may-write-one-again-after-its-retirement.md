---
id: TASK-395.14
title: >-
  Dead cells retire their references again every pass, as a parent's writer may
  write one again after its retirement
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13 18:38'
updated_date: '2026-11-14 12:53'
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
- [ ] #1 A pass over a dead cell unchanged since its last pass makes no request beyond its lease and listing
- [ ] #2 A reference written again after its retirement by a slow writer of the parent is retired or never kept: the simulation's restores profile finds no garbage left, and a direct test reproduces seed 53's order
- [ ] #3 Bucket.tla models the change, with a mutation TLC fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
