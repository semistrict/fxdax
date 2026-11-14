---
id: TASK-406.3
title: Warm a point into the part cache before a restore
status: To Do
assignee: []
created_date: '2026-11-14 12:17'
labels:
  - sandbox
  - cache
dependencies: []
parent_task_id: TASK-406
priority: low
ordinal: 795000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team asks, for later, once memory snapshots exist: a control call that reads a point's data into the host's part cache ahead of a restore, so the restore finds it local.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A control RPC takes a point and reads the objects its branch names into the part cache, within the fetch budget, as read-ahead does
- [ ] #2 A restore after it makes no bucket GET of the parts it warmed (test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
