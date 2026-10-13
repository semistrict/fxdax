---
id: TASK-26.5
title: Guest kernel driver for the virtio extent-store device
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
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
- [ ] #1 The driver binds to the device in a libkrun guest and issues every request of the specification, with many requests in flight
- [ ] #2 Extents mapped through the DAX window are readable from the guest kernel with no copy
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
