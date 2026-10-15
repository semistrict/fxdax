---
id: TASK-26.18.5
title: >-
  Extent device: serve requests concurrently and offer one request queue per
  vCPU
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
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
- [ ] #2 The device offers num_request_queues > 1 and the guest driver uses a queue per CPU
- [x] #3 extent-guest, fxfs-guest-dax, fxfs-guest-checks-dax and fxfs-guest-crash-dax pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
