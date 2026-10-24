---
id: TASK-25.8
title: 'DAX: reply to FUSE_SETUPMAPPING before KVM pre-faults the range'
status: To Do
assignee: []
created_date: '2026-10-10 20:23'
updated_date: '2026-10-24 00:53'
labels: []
dependencies: []
parent_task_id: TASK-25
ordinal: 33000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
After TASK-25.3, a 2 MiB FUSE_SETUPMAPPING takes ~580 µs in fxdax while the guest reads a cold file, ~1.1 µs a page, most of it KVM_PRE_FAULT_MEMORY, whose writes to KVM's tables nested virtualization makes slow; a range takes ~2.2 ms in all (docs/measurements/gce-fs-bench-2026-10-10T2019Z). Mapping without pre-faulting costs ~0.33 µs a page. Pre-faulting after replying, while the guest starts copying, would take it off the critical path. The hazard: a pre-fault must never reach a page the window cannot map (it would fault through userfaultfd, which aborts on an unresolvable fault), so pending pre-faults must be ordered against unmaps of their ranges (FUSE_REMOVEMAPPING, and range changes such as truncation). The other ~1.6 ms of a range is the guest's, the transport's and the L0 hypervisor's per-page faults; 2 MiB host pages behind windows (shmem THP for the physical memory, VMO pages in aligned 2 MiB runs) would let KVM and L0 map 2 MiB at a time, a larger change.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cold sequential DAX reads faster than gce-fs-bench-2026-10-10T2016Z's 862 MiB/s, measured on GCE
- [ ] #2 No pre-fault reaches a page unmapped meanwhile, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Obsolete (2026-10-23 audit of stale tasks, the user agreeing): fxdax's FUSE/virtio-fs server and files' DAX windows were removed in fda9d7d2, fxdax-extentd became the only store server in 537c7eb4, and libkrun went with TASK-40 (5ba39497). Guest Fxfs over the virtio extent device replaced them; its benchmark against ext4-dax-pmem continues under TASK-26.8 and TASK-91/92. FUSE_SETUPMAPPING is gone with it.
<!-- SECTION:NOTES:END -->
