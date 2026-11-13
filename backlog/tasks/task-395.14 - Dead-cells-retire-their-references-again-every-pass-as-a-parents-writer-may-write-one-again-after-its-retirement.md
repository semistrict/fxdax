---
id: TASK-395.14
title: >-
  Dead cells retire their references again every pass, as a parent's writer may
  write one again after its retirement
status: To Do
assignee: []
created_date: '2026-11-13 18:38'
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
