---
id: TASK-26.5
title: Guest kernel driver for the virtio extent-store device
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-13 19:54'
updated_date: '2026-10-14 03:47'
labels: []
milestone: m-3
dependencies:
  - TASK-26.1
  - TASK-26.2
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: feature
ordinal: 41000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest side of the extent-store device (doc-2): a driver in libkrunfw's kernel that exposes the protocol to the in-kernel Fxfs, including mapping the device's DAX window so file pages are read straight from the host's shared pages. Built against the kernel version chosen by the porting spike.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The driver binds to the device in a libkrun guest and issues every request of the specification, with many requests in flight
- [x] #2 Extents mapped through the DAX window are readable from the guest kernel with no copy
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. guest/linux/drivers/virtio_extent: a virtio driver for device 0x4658: config, one queue, requests submitted asynchronously (many in flight) with completions, sync helpers, the DAX window memremapped; an in-kernel API (include/linux/virtio_extent.h) for the guest's Fxfs.
2. A self-test in the driver, run through a sysfs attribute: INFO, concurrent WRITEs, COMMIT with root, ROOT, concurrent READs, a sealed write's EPERM, WRITE_BARRIER, MAP and reading the window directly (no copy), UNMAP, FREE, ENOENT.
3. build-guest-kernel.sh builds the driver in; libkrun test extent-guest boots the guest kernel with an extent-store device and runs the self-test.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed on branch object-store (local): b852fc0 guest/linux/drivers/virtio_extent (driver, in-kernel API in include/linux/virtio_extent.h, sysfs self-test) built in by build-guest-kernel.sh; libkrun 14e17d7 test extent-guest. Evidence: extent-guest passes on macOS HVF arm64: the self-test issues INFO, 32 concurrent WRITEs, COMMIT+ROOT, ROOT, 32 concurrent READs, sealed-write EPERM, WRITE_BARRIER, MAP + reading the DAX window in place with zero padding, UNMAP, FREE + ENOENT, final durable COMMIT; the host then reopens the store and finds the root, used bytes and next sequence the guest committed. fxfs-guest still passes on that kernel. Not yet run on x86-64 KVM. DoD #1 waits on the obsolete Fxfs test, as TASK-26.3's.
<!-- SECTION:NOTES:END -->
