---
id: TASK-395.1
title: >-
  Spec: Bucket.tla collects per cell, with fork references and conservative
  fork-point keeping
status: To Do
assignee: []
created_date: '2026-11-13T08:52:00Z'
labels:
  - bucket
  - collector
  - spec
dependencies: []
parent_task_id: TASK-395
priority: high
ordinal: 773000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Bucket.tla models one collector over a flat set of branches: one lease, one atomic listing (`GcStart`), records read until none are new, every branch recovered, compaction and deletes in one step. A per-cell collector breaks the rules the current proof relies on (candidates listed first over the whole bucket, records relisted bucket-wide), so the protocol is model-checked before any code changes. The model is the design of record for the sibling tasks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Branches belong to cells; each cell has its own collector lease and passes, which list and delete only under the cell, running concurrently with other cells' passes and with writers
- [ ] #2 A fork of a branch in another cell has a reference under the parent's cell, written before anything the child's survival depends on is published (a writable fork's child record; a point clone's forking record, before it reads the point's record), removed only once the child is deleted and its own cell holds no references
- [ ] #3 A cell's pass keeps, for each reference, everything the parent's state at the fork point needs; and lists its references again after reading its records, as passes relist records today
- [ ] #4 The invariants that hold today (no live branch loses what it reads; creates never land where the collector freed) hold with per-cell passes, under the existing cfgs extended with two or more cells and cross-cell forks, point clones and deletions
- [ ] #5 A mutation for each new rule (no reference, reference written late, reference removed while descendants live, no relisting of references, fork-point state not kept) makes TLC find a violation
- [ ] #6 doc-7's Garbage collection section describes the per-cell protocol
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
