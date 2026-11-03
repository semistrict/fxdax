---
id: TASK-160.8
title: 'extentd: one table owns each store path''s lifecycle'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 579000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Who holds the store at a key, and as what, has five owners: open_paths(), instances() with its retired flag, points(), the connection's opening/store/path fields, ExtentStore::following. The daemon's hold releases on drop; a connection's by hand in two places. Explains TASK-79.34, 150.5, 54, 79.84, 79.95, 92.5, 79.103, 79.36. Proposal: StoreTable keyed by store_key, each entry one enum (Held, Writer with followers, Followed, kept point), every hold a value released on drop; the connection a Phase. Widen open_stores' Shuttle model to followers, retirement and points.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One table and one connection phase
- [ ] #2 A test of concurrent OPENs as the VM process exits
- [ ] #3 TASK-79.34's and 150.5's tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
