---
id: TASK-37.2
title: 'extentd: RESIZE request and the VM''s capacity changes'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
labels: []
dependencies: []
parent_task_id: TASK-37
type: feature
ordinal: 107000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A management request, RESIZE (path, capacity), on a connection that has not opened a store, finds the connection holding the store (a path-to-connection registry in place of open_paths). Grow: the store's capacity first, then CAPACITY_CHANGED to the VM process. Shrink: CAPACITY_CHANGED asking the guest first; the VM process answers with what the guest took (a new reply path), and the store shrinks only then. The client (RemoteExtentStore) delivers CAPACITY_CHANGED to a callback the VMM registers. fxdax-branch resize sends RESIZE when the daemon has the store open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 RESIZE grows an open store and notifies its VM process; shrinks it once the VM process accepts, and refuses when it doesn't, shown by extentd tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
