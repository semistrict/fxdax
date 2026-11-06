---
id: TASK-149.24
title: >-
  Locks shared beyond one store poison the daemon on a panic: the extent
  cache's, a window's, a connection's
status: To Do
assignee: []
created_date: '2026-11-03 07:40'
updated_date: '2026-11-06 00:45'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
type: task
ordinal: 570000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-153.16's look-around. Store locks (ExtentStore's) now fence their store on a panic (crate::fence), and the daemon fences a store whose request, flush or clean panicked. Not covered, each still std Mutex with unwrap, at dde8d58d3:

(1) The process-wide extent cache's table (crates/fxdax/src/cache.rs:38, ExtentCache::extents) and its holders: a panic under it would poison every tenant's reads, not one store's; fencing one store doesn't help, so the choice is aborting the daemon (systemd restarts it) or proving the sections can't panic.
(2) A window's state (crates/fxdax/src/dax.rs:548, Window::state), shared with the connection's DaxWindow: DaxWindow::maps_pages (dax.rs:290) and fault serving (Window::fault, dax.rs:612) unwrap it; teardown skips a fenced store's window (reaches_pages, crates/fxdax/src/extentd/server.rs:526), but a fault on a window poisoned by a panic under Window::state alone would panic again.
(3) A window's change ends from Change's Drop (dax.rs:188-201), during unwinding too: RemoteStage::end_change (server.rs:2183-2189) locks the stage's change, unwrapped, and expects one begun, so a poisoned lock or a change never begun panics in a drop, aborting the process. (Since 92ff6fc16 the stage holds the change, not a thread-local of the wire's; end_deferring went with writable DAX, 37c331a6c.)
(4) A follower's ExtentStore shares the writer's extent_store::Store: a panic under Store's state lock fences the store whose request panicked, and each follower takes one panic of its own before the daemon fences it.
(5) extent_store's own locks could take crate::fence's pattern, fencing the Store for all of its ExtentStores; not done in TASK-153.16 to keep out of TASK-158's changes there.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test panics under each lock listed and shows the daemon either fences what it must or stops cleanly, never one panic per later request and never an abort from a drop
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: citations brought to dde8d58d3; item (3) restated for 92ff6fc16, which moved a window's held-back change into its RemoteStage.
<!-- SECTION:NOTES:END -->
