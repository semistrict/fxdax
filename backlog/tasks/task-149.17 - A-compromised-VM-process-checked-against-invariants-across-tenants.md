---
id: TASK-149.17
title: 'A compromised VM process, checked against invariants across tenants'
status: To Do
assignee: []
created_date: '2026-11-01 16:53'
labels: []
dependencies:
  - TASK-77
parent_task_id: TASK-149
priority: high
type: task
ordinal: 534000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-77 fuzzes the wire protocol and asks that nothing panics and the daemon serves on. Three audit findings pass that: they need stating what must stay true. Two tenants, one hostile, each request sequence followed by checks of the other.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Random request sequences, on one connection and concurrent ones, from one tenant of two
- [ ] #2 The daemon memory stays within its bound for the connection
- [ ] #3 The other tenant commits, reads its own data, and no path stays held
- [ ] #4 A store used only as the protocol allows keeps committing
- [ ] #5 A bug of each kind, put back, fails it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
