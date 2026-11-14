---
id: TASK-395.13
title: >-
  A fork's reference written by a recorder whose read of the child's record is
  stale, dying before its create, is never retired
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13 18:27'
updated_date: '2026-11-14 12:02'
labels:
  - bucket
  - collector
  - bug
dependencies: []
parent_task_id: TASK-395
priority: low
ordinal: 786000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-395.10's strict oracle (testing::uncollected flags a reference no record names): sim restores seed 17, and seeds of branches_deleted_twice_while_forking, collectors_stopping_and_racing, takeovers_while_uploading. A writer of the parent records a fork across cells: branch::record_fork reads the child's record (none), writes the parent's checkpoint at the fork point, and, before it writes the reference, the child gets a record that names no origin or another (branch::delete of a child with no record creates a deleted Kind::Own; another branch takes the ID), and another recorder (the parent's deletion, its next writer) reads that and takes the reference back (rule references_taken_back, nothing there yet). The first writer then writes the reference and is stopped (host crash, session stopped) before its create, which would have taken it back. No record names the reference; nothing looks at it again; its fork point is kept for ever (space, never data). TASK-395.10 narrowed the window to one round trip (the record read again after the checkpoint), and the model merges the read with the write (specs/bucket/Bucket.tla, its header). No finite set of later lookers closes it: a later observer is needed. Candidates: (1) the recorder writes a marker naming the parent and fork point in the child's cell before the reference, which the child's cell's pass, its cell dead, retires by as by records' origins (a PUT per fork across cells); (2) writable forks as clones are since TASK-395.10: the child's record first, pending and naming the origin, then the reference, then the record made live (a protocol change: writers, hosts waiting on records, claims, deletions, the collector). testing::uncollected excuses exactly this case naming this task: a reference whose child's record is there but names no origin or another.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A reference never outlives every record of its child naming another origin or none, whatever recorder dies when (a model of the code's separate read and write shows it)
- [ ] #2 testing::uncollected no longer excuses references whose child's record names another origin
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
