---
id: TASK-164.11.1
title: >-
  The VMM's virtqueue request path costs ~165 us a request, every request's:
  find where it goes and cut it
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-06 06:26'
updated_date: '2026-11-06 23:19'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-06, measured with scripts/extent-transport.bt (FXDAX_HOST_TRACE, test-linux-host.sh), GCE r800-r803, nested, jailed, forked pgbench with DAX. A PRIVATE takes the guest 527 us (r802, traced): the VM process waits 282 us for the daemon's reply (daemon 66 us, the stage run 116, ~100 the messages between them), the other ~245 us are the transport's and the guest's. Seen from the host: an MMIO exit 8-32 us of its time (5.3M in r800; 1.73M of r802's 2.68M went to the VMM, the rest in-kernel), the interrupt's handling two of those to the VMM (virtio-mmio's status read and ACK, which no MSI spares: the PCI transport has no shared memory regions, which the window is), a HLT, then the wakeup (an interrupt to the vCPU's entry 4-32 us); the threads' waits to run 1-4 us mostly, the daemon's extentd io threads' with a tail past 16 us. Tried: halt polling in the guest (cpuidle_haltpoll.force=1, r801: forked-dax 0.826, the halts last 128-512 us, past its window) and a guest spinning for a PRIVATE's reply (virtio_extent.spin_ns=600000, r803: 0.843, a PRIVATE 432 us from 463): neither moved forked-dax past noise. AC1 partly: each host hop measured as above; the VMM's own part (worker pop to completion, outside the daemon's wait) not apart, its served times logged at Info as the worker stops, inside the jail that results leave out. No longer needed for TASK-164.11's 0.9 (r807: 0.912 with fewer PRIVATEs, made before the inode's lock); left as later work.
<!-- SECTION:NOTES:END -->
