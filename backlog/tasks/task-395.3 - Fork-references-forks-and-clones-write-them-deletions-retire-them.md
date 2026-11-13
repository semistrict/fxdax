---
id: TASK-395.3
title: 'Fork references: forks and clones write them, deletions retire them'
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
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
- [ ] #1 A writable fork's child record, by whoever writes it (the fork's host, the parent's writers recovering, the parent's deletion), is preceded by its reference, of bytes unique to the fork (its owner) so that every writer writes the same
- [ ] #2 A clone of a point writes its reference before its forking record, and before reading the point's record
- [ ] #3 A deleted child's reference is removed by its cell's collector pass only once the child's cell holds no references itself
- [ ] #4 The simulation (extent_remote sim) and crash rounds cover a host dying between each pair of steps: no live branch loses data, and a reference without a child is retired
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
