---
id: TASK-43.6
title: >-
  extentd: store operations wait without holding a thread (uploads, resize
  answers, store locks)
status: To Do
assignee: []
created_date: '2026-10-18 05:15'
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
- [ ] #1 Waiting for uploads (`wait_uploaded`, SyncStore, ForkStore of a branch with pending uploads) is a future woken by the uploader, holding no thread while it waits
- [ ] #2 ResizeStore waiting for the guest answer and waiting for the cross-process store lock are futures, holding no thread while they wait
- [ ] #3 A test runs many more concurrent waiting SyncStore calls than the blocking pool has threads, while another RPC on the same daemon completes promptly
- [ ] #4 Deadlines and cancellation unchanged: a dropped RPC stops waiting, and timeouts report as before
- [ ] #5 Disk I/O stays on the blocking pool; no async wrapper hides a blocking call on a runtime thread
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
