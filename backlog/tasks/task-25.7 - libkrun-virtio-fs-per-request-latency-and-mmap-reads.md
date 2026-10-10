---
id: TASK-25.7
title: 'libkrun virtio-fs: per-request latency and mmap reads'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 21:25'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 32000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Over libkrun's virtio-fs, with fxdax or passthrough, creating, removing and listing files is 20-60x slower than ext4 and mmap reads ~100x (docs/measurements/gce-fs-bench-2026-10-10T1823Z.md): a request costs 0.1-0.7 ms on GCE under nested KVM, in the guest's notification, the fs worker's wakeup and the interrupt back. And a file's cached pages may be dropped once the guest writes it (AUTO_INVAL_DATA seeing the changed mtime), so mmap reads after writes go to the device. Find where a request's time goes, and cut it: e.g. polling the queue before sleeping, or the writeback cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Where a virtio-fs request's time goes on GCE is measured and recorded
- [ ] #2 Creates, unlinks and mmap reads over fxdax faster than in docs/measurements/gce-fs-bench-2026-10-10T1823Z.md, measured on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count KVM's events per request (exits, exits to the VMM, MMIO, wakeups), fxdax against passthrough. 2. Time a request in each device. 3. Count and record the VMM's context switches, and where they happen. 4. Test the vCPUs' halt polling's part. 5. Record the findings; measure creates, unlinks and mmap reads against the baseline.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Where a request's time goes on GCE (runs T2111Z to T2122Z): (1) KVM: per request ~2 exits to the VMM (virtio-mmio's interrupt handler reading and acknowledging the interrupt status), ~7 MMIO accesses (with the queue notification, an ioeventfd), ~1 vCPU wakeup, the same for fxdax and passthrough. (2) In the device, from taking a request off the queue to signalling its reply: fxdax 17-22 µs, passthrough 7-10 µs. (3) Context switches per cold stat: fxdax ~3.5, passthrough 2; the device thread switches out in epoll_wait and right after writing the interrupt eventfd, preempted by the vCPU it wakes (39% of fxdax's switches, 46% of passthrough's); fxdax's tokio worker also waits on futexes (11%). (4) Halt polling: off, passthrough's cold stats fall from ~9,900 to ~7,100 a second while fxdax's stay at ~5,100: passthrough's replies come while the vCPU polls (up to 200 µs), fxdax's too late, so its vCPU sleeps and is woken each time. ~40 µs per request remains unaccounted for. Ideas: make fxdax's replies quicker than the halt-poll window (the ~12 µs extra in the device; its futex waits), or poll the queues briefly in the device after a reply; virtio-mmio's two exits per interrupt could go with virtio-pci/MSI-X.
<!-- SECTION:NOTES:END -->
