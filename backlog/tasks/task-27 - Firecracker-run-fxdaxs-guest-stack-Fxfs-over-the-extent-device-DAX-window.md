---
id: TASK-27
title: 'Firecracker: run fxdax''s guest stack (Fxfs over the extent device, DAX window)'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 23:42'
updated_date: '2026-10-16 01:41'
labels: []
dependencies: []
priority: high
type: feature
ordinal: 75000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user decided to move fxdax onto Firecracker (fork semistrict/firecracker, branch fxdax, submodule vendor/firecracker), keeping libkrun meanwhile. fxdax's guest stack (guest kernel with Fxfs over the virtio extent device, its DAX window served by fxdax-extentd) runs only under libkrun today. Firecracker has no virtio shared memory regions, no virtio-fs, and its own device model, config API and seccomp filters.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A guest kernel with Fxfs mounts an extent store through Firecracker and passes the guest suites (extent-guest, fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax) on GCE
- [x] #2 pjdfstest passes on the guest Fxfs (with and without DAX) under Firecracker
- [x] #3 The fs bench runs its fxfs-guest-dax and ext4 cases under Firecracker, saved in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax's guest stack runs under Firecracker (fork semistrict/firecracker, branch fxdax) as well as libkrun: guest suites and pjdfstest (with and without DAX) pass on GCE x86-64, the fs bench is saved (gce-fs-bench-2026-10-16T0121Z). Workspace tests pass on macOS (1873) and Lima (1879); upstream-changes --check passes. Deviations: Firecracker's fork changes are behind its fxdax feature, but for the MMIO shm registers, KvmMemSlot and VirtioDeviceType's width.
<!-- SECTION:FINAL_SUMMARY:END -->
