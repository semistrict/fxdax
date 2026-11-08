---
id: TASK-149.24
title: >-
  Locks shared beyond one store poison the daemon on a panic: the extent
  cache's, a window's, a connection's
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 07:40'
updated_date: '2026-11-08 11:22'
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
- [x] #1 A test panics under each lock listed and shows the daemon either fences what it must or stops cleanly, never one panic per later request and never an abort from a drop
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_store::shared: SharedLock (locked only through with), Shared guard tracking the thread's held locks, abort_on_panics hook aborting on a panic of a thread holding one; testing::panic_holding.
2. Every lock the task lists, and the connection's and the store's others, a SharedLock; fxdax-extentd installs the hook.
3. A child-process test per lock: SIGABRT naming it; red without the hook; a mutant.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: citations brought to dde8d58d3; item (3) restated for 92ff6fc16, which moved a window's held-back change into its RemoteStage.

2026-11-08: the user's decision (b0d7d00d2): abort rather than poison. land-h14 61a8dda62, CI 42449893885. Twelve locks covered, each by a child daemon in a_panic_holding_a_shared_lock_stops_the_daemon: extent cache, cached extent holders, cached extent pages, window, stage change, connection phase/window/requests, store state/commit/durability/flush; also shared but untested alone: extent cache arenas, promoted extent, connection behind, store opening and log base. Red without the hook: the extent cache's panic fenced one store, the fault after died EIO. The hook aborts at the panic, before unwinding, so no drop runs under a poisoned lock; end_change no longer panics in a drop during a panic. Outside the daemon (tests, other processes) nothing changes. Docker (Linux): fxdax + extent_store 707/711 (4 need git or sudo, absent in the container), shuttle parts as CI runs them green, clippy's disallowed_methods clean; macOS 483/483; fxdax mutants: the new one and four rebased onto server.rs killed; --check 16/16.

Verified 2026-11-08: on main, GitHub CI 42469441083 green at 08d444ef7 (macOS and Linux workspace, shuttle, sim, guest-sim, firecracker), scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A panic holding a lock shared beyond one store aborts the daemon at the panic, naming the lock, rather than poisoning it (the user's decision: abort rather than poison); twelve locks are each covered by a child daemon in a_panic_holding_a_shared_lock_stops_the_daemon, red without the hook (3afa08d64).
<!-- SECTION:FINAL_SUMMARY:END -->
