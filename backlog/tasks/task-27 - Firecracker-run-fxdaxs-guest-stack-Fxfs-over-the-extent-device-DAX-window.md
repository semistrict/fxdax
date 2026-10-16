---
id: TASK-27
title: 'Firecracker: run fxdax''s guest stack (Fxfs over the extent device, DAX window)'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-15 23:42'
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
- [ ] #1 A guest kernel with Fxfs mounts an extent store through Firecracker and passes the guest suites (extent-guest, fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax) on GCE
- [ ] #2 pjdfstest passes on the guest Fxfs (with and without DAX) under Firecracker
- [ ] #3 The fs bench runs its fxfs-guest-dax and ext4 cases under Firecracker, saved in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
