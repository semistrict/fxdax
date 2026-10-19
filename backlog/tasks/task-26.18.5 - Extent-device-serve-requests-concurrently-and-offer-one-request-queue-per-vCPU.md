---
id: TASK-26.18.5
title: >-
  Extent device: serve requests concurrently and offer one request queue per
  vCPU
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 04:05'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: medium
type: enhancement
ordinal: 59000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
libkrun served the one request queue on one thread, in order, though doc-4 lets requests on all queues apply in any order but across barriers; a MAP waited behind writes and their durable commits. A pool of servers is written (vendor/libkrun extent/worker.rs, uncommitted at task creation; correctness cases passed on GCE). The device still offers one queue, so the guest serializes submissions on one lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The host serves requests of one queue concurrently, completing them out of order
- [x] #2 The device offers num_request_queues > 1 and the guest driver uses a queue per CPU
- [x] #3 extent-guest, fxfs-guest-dax, fxfs-guest-checks-dax and fxfs-guest-crash-dax pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. libkrun extent worker: a thread takes requests off the queue, a pool of 8 serves them (done in tree; correctness cases passed on GCE).
2. Commit it with the driver's request_ns counters.
3. Multiple request queues: later, after measuring whether submission contention matters.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
libkrun d9acc34: a thread takes requests off the queue, 8 serve them. Suites pass on GCE. MAP latency unchanged (~0.85 ms): the MAP itself is slow (TASK-26.18.3). Multiple queues: not done.

Multiple queues (2026-10-19, d9911be, Firecracker b040c6c0f): the device offers 4 request queues; the driver sets up one per CPU, up to that (struct vext_queue: its own lock, room and request count; sysfs queues shows each one's requests); Firecracker runs a worker per ready queue with its share of 8 servers, and initializes ready queues alone on activation, dirty-marking and restore. fxfs-guest's queue_per_cpu pins a writer to each CPU (2 in the test VM) and sees every queue take requests. All 18 guest cases pass on GCE, the snapshot ones included (they found the unready queues: activation, dirty-marking, restore). Firecracker's virtio unit tests pass but for balloon/net/block ones that vary from run to run on the host (untouched).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The host serves requests of a queue concurrently (libkrun d9acc34, then Firecracker), and the device offers 4 request queues, the guest's driver using one per CPU, each served by a worker of its own. Verified by fxfs-guest's per-CPU check and all 18 guest cases on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
