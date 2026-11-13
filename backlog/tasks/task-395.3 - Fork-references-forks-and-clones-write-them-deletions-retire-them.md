---
id: TASK-395.3
title: 'Fork references: forks and clones write them, deletions retire them'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 20:33'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.1
  - TASK-395.2
parent_task_id: TASK-395
priority: high
ordinal: 775000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A per-cell collector keeps what forks in other cells inherited only if it can see them under its own prefix (the parent's footers name writable forks, but nothing in the parent's cell records point clones, nor when a child goes). The writers of child records write a reference first; the child's deletion leads to its retirement. Ordering as model-checked in the spec task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A writable fork's child record, by whoever writes it (the fork's host, the parent's writers recovering, the parent's deletion), is preceded by its reference, of bytes unique to the fork (its owner) so that every writer writes the same
- [x] #2 A clone of a point writes its reference before its forking record, and before reading the point's record
- [x] #3 A deleted child's reference is removed by its cell's collector pass only once the child's cell holds no references itself
- [ ] #4 The simulation (extent_remote sim) and crash rounds cover a host dying between each pair of steps: no live branch loses data, and a reference without a child is retired
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rules: references, references_first, clones_refer_first (spec's last six become fields, with 395.4's three).
2. branch::record_fork: a child of another cell than its parent's gets its reference first (create-only, bytes Reference{format, owner}), whoever writes the record (writer, next writer, deletion); failpoint branch::referred between. Without references_first: after.
3. branch::fork_point: the reference first, whatever cell, before the forking record and the point's read; failpoint point::referred. Without clones_refer_first: once made.
4. Retirement in the child's cell's pass (395.4): Pass::retired, keys from deleted records' origins in other cells, once the cell has no live branch and no reference.
5. Crash coverage: FORK_STEPS/RESTORE_STEPS gain branch::referred / point::referred (fxdax fork_faults/restore_faults die/fail/race at each step); sim: every fork crosses cells (sim_branch), passes per cell.
6. Tests: references written before records (and not within a cell), owners' bytes, mutations of each rule, retirement, sim test_needs where a seed finds it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), batch with TASK-395.2 and .4 on branch t3/395-per-cell-collector.
Landed:
- Rules: references, references_first, clones_refer_first (and .4's references_held, references_relisted, fork_points_kept): the spec's last six are fields now, each with a test that turns it off.
- branch::record_fork, the one writer of a writable fork's child record (the fork's writer, its next writer, the parent's deletion): for a child of another cell, reads the child's record, then writes the reference `<C>refs/<parent>/<at>/<child>` (bytes Reference{format, owner}: the fork's owner, so every writer writes the same), failpoint branch::referred, then creates the record from what it read. A child of the parent's own cell gets none (its cell's pass reads its record).
- branch::fork_point (every clone, whatever cell, its origin's own too): the reference first, failpoint point::referred, then the forking record, then the point's read.
- Rule off: references_first: no reference in record_fork; the writer writes it after recording, the deletion after marking the parent deleted (as NoReferencesFirst). clones_refer_first: once the fork is made.
- Retirement: the child's cell's pass (TASK-395.4, collector::Pass::retired).
- extent_store Remote::unfork of a point's copy deletes the failed clone as Kind::Forking{origin, point}, so its deleted record names the origin and its reference retires (it was Kind::Own, naming none: found by restore_faults).
- fxdax faults: FORK_STEPS gains branch::referred, RESTORE_STEPS point::referred, so fork_faults/restore_faults fail, kill the host, race passes and take IDs at each reference step.
Where code differs from the spec:
- record_fork reads the child's record before the reference, and writes none for a child deleted or another's: nothing inherits through it, and nothing would retire it (fork_faults found both: a fork deleted before its host knew its origin, recorded Own; an ID taken). The model writes it whatever the record. A GET more per fork across cells.
- References that are never retired (a clone's host dying between its reference and its record; an ID taken between the read and the create): space, never data, as the model too has them. Filed TASK-395.10; restore_faults/fork_faults excuse exactly those, naming it.
Tests: collector::tests writable_forks_refer_before_their_childrens_records, clones_refer_before_their_records_and_their_points_read, deletions_refer_before_they_mark_parents_deleted (+ _need_references_first), clones_keep_what_they_inherit_as_their_points_go (+ _need_clones_refer_first), references_hold_while_their_childrens_cells_hold_references (+ _need_references_held), fork_points_keep_what_children_in_other_cells_inherit (+ _need_references), dry_runs_and_mark_only_passes_retire_nothing; mutant patch forks-take-branches-claimed re-anchored to the read-first refusal.

Verified on macOS (branch t3/395-per-cell-collector at 675fd333b, rebased on origin/main c6ba0d117): scripts/test.sh before the rebase exit 0: clippy clean, workspace 2315/2315, sim 59/59, Shuttle 37+10+33+6+10+18 all passed, scripts 312 OK, mutant checks 0 broken, go ok. After it: clippy gate clean, nextest extent_remote+extent_store+fxdax 807/807, sim 59/59, mutant checks 0 broken. Commits 1d07b12e8 (code and tests), a05be7dc2 (doc-7, OPEN_QUESTIONS.md), 675fd333b (backlog). Not run: ubu1 (the parent's), TLA+ (specs untouched). Flake seen once under load, outside this change: TASK-399.

AC #4 left unchecked: the simulation and the fault rounds cover a host dying at each reference step and lose nothing, but a reference without a child is not always retired: a clone's whose host died between its reference and its record stays (TASK-395.10, excused by name in restore_faults).

Review round (@claude, 2026-11-13; a6cc31ba1, 0de908c63): a clone refused by a record of its child's ID (Deleted, IdTaken) deletes the reference it wrote, test clones_refused_by_records_there_take_their_references_back; TASK-395.10's description has the case. fork_faults/restore_faults excuse exactly what a left reference keeps (testing::fork_point_keeps). record_fork re-puts the reference of a child recorded already (as the model), skipping the checkpoint's recovery; a child whose fork point's state is gone is recorded deleted with no reference (TASK-395.11).

ubu1 run t395w3 on bb07304a6 (under fxjob): workspace 2596/2596 passed (90 skipped), sim 59/59, the other nextest parts all passed, mutant checks 0 broken, guest tests exit 0. Merged to main as 1c1174bb4 (rebased over backlog-only commits).
<!-- SECTION:NOTES:END -->
