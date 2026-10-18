---
id: TASK-43.6
title: >-
  extentd: store operations wait without holding a thread (uploads, resize
  answers, store locks)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-18 05:15'
updated_date: '2026-10-18 15:22'
labels: []
dependencies:
  - TASK-43.3
parent_task_id: TASK-43
priority: medium
type: feature
ordinal: 137000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-43.3 runs the gRPC API on tokio and puts the blocking store operations on `spawn_blocking`. Several of them block not on I/O but on waits that can last seconds to unbounded: SyncStore and ForkStore wait for uploads (a Condvar the uploader signals; unbounded while the network is slow or down), ResizeStore waits up to 10 s for the guest to answer, and DeleteStore and others wait up to 10 s for the cross-process store lock (an flock polled). Each such wait parks a blocking-pool thread, so many concurrent syncs or forks (e.g. a fleet snapshot during an outage) exhaust the pool and stall unrelated RPCs. The waits are sync only for history: extent_store began as a sync library under a thread-per-connection daemon. Disk I/O (fsync-heavy writes, reads of segments) has no cheap async form without io_uring and stays synchronous behind the blocking pool; this task is only about the waits.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Waiting for uploads (`wait_uploaded`, SyncStore, ForkStore of a branch with pending uploads) is a future woken by the uploader, holding no thread while it waits
- [ ] #2 Deadlines and cancellation unchanged: a dropped RPC stops waiting, and timeouts report as before
- [x] #3 Disk I/O stays on the blocking pool; no async wrapper hides a blocking call on a runtime thread
- [x] #4 ResizeStore waiting for the guest answer, and waiting for a connection to let a store go (the in-process store lock), are futures, holding no thread while they wait; the cross-process lock (flock) is TASK-43.8
- [x] #5 A test runs many more concurrent waiting calls (shrinks whose guests never answer) than the blocking pool has threads, while another RPC on the same daemon completes promptly
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. remote.rs: the uploader's waits on an event_listener::Event (blocking wait() and async await); Remote::uploaded(epoch) future.
2. ExtentStore::durable_epoch (flush, blocking) and ExtentStore::uploaded(epoch) (future).
3. open_stores::Paths::hold_async: the in-process store lock waited for with an Event and a deadline.
4. stores.rs: the control API's operations async: Held::acquire, blocking() for I/O, Here (a store opened on, and closed on, a blocking thread); fork and sync wait for uploads holding no thread, then fork on a blocking thread.
5. Connection::resize async: the guest's answer a oneshot, awaited with tokio timeout.
6. control.rs awaits the operations; the blocking pool capped at BLOCKING_THREADS (32).
7. Test: 48 shrinks whose guests never answer, a StatStore completing within 2 s.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verified: GCE 2070 tests and 6 guest cases (resize, live fork, migrate, snapshot), macOS 2027. Red check: with the shrink waiting on a blocking thread (as before), shrinks_waiting_for_their_guests_hold_no_threads fails (only 32 of 48 shrinks start).

Not held by a thread now: waits for uploads (sync; fork's wait for the backlog), guests' shrink answers, in-process store holds. Still on a blocking thread: the fork's own last step (fork_host, and the uploader's answer to the fork, one segment's upload once the backlog is drained), opening a store no VM has open (attach, which uploads what the store holds that the bucket lacks), closing one (Remote::drop uploads what is durable), and the cross-process flock wait (TASK-43.8).

AC 2 (cancellation): tonic drops a cancelled call's future, which stops its waits (Held released on drop); timeouts report as before (ETIMEDOUT shown by the new test). The cancellation itself has no test.
<!-- SECTION:NOTES:END -->
