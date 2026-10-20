---
id: TASK-54
title: 'extentd: deleting a store just after its VM process closes can fail EBUSY'
status: To Do
assignee: []
created_date: '2026-10-20 18:30'
labels:
  - extentd
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 162000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once in 2240 Linux workspace tests (GCE, TASK-53's verification run): extentd::tests::stores_are_deleted_while_their_forks_are_open got EBUSY from delete_store right after dropping the fork's client (crates/fxdax/src/extentd/tests.rs:903). Suspected cause: Connection::run's pool task notifies 'drained' (requests.running hits 0) while still holding its Arc<Connection>; serve()'s teardown wakes, finish_connection drops its own Arc (not the last) and releases the path, and a delete then finds the store instance still upgradeable (Held::alone -> instance() -> EBUSY) until the task's closure returns.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The race is reproduced by running the test many times on Linux, at a measured failure rate
- [ ] #2 The connection's store is gone before its path is released, and the test passes as many runs without a failure
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
