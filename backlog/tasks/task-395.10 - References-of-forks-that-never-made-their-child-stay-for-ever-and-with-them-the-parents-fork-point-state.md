---
id: TASK-395.10
title: >-
  References of forks that never made their child stay for ever, and with them
  the parent's fork-point state
status: To Do
assignee: []
created_date: '2026-11-13 13:47'
updated_date: '2026-11-13 15:26'
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
- [ ] #1 A clone whose host dies between its reference and its record leaves nothing once its directory is deleted and passes ran
- [ ] #2 fork_faults and restore_faults no longer excuse references left behind
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
