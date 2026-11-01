---
id: TASK-149.18
title: 'Simulator invariants: every acknowledged object exists, every waiter is woken'
status: To Do
assignee: []
created_date: '2026-11-01 16:53'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 535000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_remote sim checks what its runs read back; a loss shows only if a read reaches it, and a waiter never woken shows only as a hung run, if at all. Stating both as invariants, and generating own extents and branches of children, reaches what the audit traced by hand. The extent_store side needs a Shuttle or quiescence check of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 After each step, every object an acknowledged segment names exists, or is known lost by the rules
- [ ] #2 Runs include children of their own branch uploading inherited extents of 4 MiB or more
- [ ] #3 At quiescence no upload, sync or durable commit still waits
- [ ] #4 The collector and upload bugs, put back, each fail it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
