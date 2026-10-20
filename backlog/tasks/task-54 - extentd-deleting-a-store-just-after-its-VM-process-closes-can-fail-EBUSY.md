---
id: TASK-54
title: 'extentd: deleting a store just after its VM process closes can fail EBUSY'
status: Done
assignee: []
created_date: '2026-10-20 18:30'
updated_date: '2026-10-20 19:03'
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
- [x] #1 The race is reproduced by running the test many times on Linux, at a measured failure rate
- [x] #2 The connection's store is gone before its path is released, and the test passes as many runs without a failure
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fec30d4. Unwidened, 300 stress iterations passed (rare without load). Widened with a 50 ms sleep after 'drained' in the old order: 20/20 iterations failed with EBUSY; the fix (drop the connection, then notify) with the same sleep after notifying: 20/20 passed. Full Linux workspace (2243) passes with the fix.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Request tasks let the connection go before telling its teardown they're done, so its store is gone before its path is released; confirmed by a widened window (20/20 EBUSY before, 20/20 pass after).
<!-- SECTION:FINAL_SUMMARY:END -->
