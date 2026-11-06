---
id: TASK-164.11.1
title: >-
  The VMM's virtqueue request path costs ~165 us a request, every request's:
  find where it goes and cut it
status: To Do
assignee: []
created_date: '2026-11-06 06:26'
updated_date: '2026-11-06 06:30'
labels:
  - performance
  - dax
dependencies: []
parent_task_id: TASK-164.11
priority: medium
type: task
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured on GCE r650 (n2-standard-4, nested, jailed), pgbench on a fork with DAX, a minute: a PRIVATE costs the guest ~486 us (window_privates_ns), of which the VM process waits 320 us for the daemon's reply (vm-private-request-ns: the daemon 83 us, its own stage run 135, ~100 between them), leaving ~165 us on the request's way through the VMM: the guest's notify, the extent device's queue thread, its async request, and the completion and interrupt back. Every request takes that way, not only PRIVATE: a WRITE is 59 us in the daemon (write-ns) and ~385 us in the guest (device_writes_ns), and no DAX's pgbench pays it on every WRITE, READ and COMMIT. Where it goes is unmeasured: thread hops and their wakeups (eventfd, the executor, the extentd client's receiving thread, which also makes the stage operations), copies of the descriptors' data, the interrupt's delivery to a halted vCPU, nested.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where a request's time between the guest's notify and its completion goes is measured, each hop apart, for PRIVATE and WRITE
- [ ] #2 A request's time in the VMM, outside the daemon's wait, at most 80 us nested (from ~165), measured as the AC above does
- [ ] #3 forked-dax and dax (the perf gate's single phase) measured on GCE before and after, neither worse
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
