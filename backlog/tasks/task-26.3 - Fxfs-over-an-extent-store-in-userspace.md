---
id: TASK-26.3
title: 'Fxfs over an extent store, in userspace'
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
labels: []
milestone: m-3
dependencies:
  - TASK-26.2
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: feature
ordinal: 39000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fxfs's Device and allocator assume a fixed-size block device whose ranges it allocates; the device size is read once at open (crates/fxfs/src/object_store/allocator.rs, device_size) and nothing grows it. In the design (doc-2) storage is an extent store instead: Fxfs allocates extents by ID, records IDs in its tree, maps and frees them, and storage grows and shrinks with use. Doing this in userspace first, where Fxfs runs today, separates the format and allocator change from the kernel port.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fxfs allocates, records, reads and frees data and its own metadata through an extent-store interface rather than device ranges, with no fixed device size
- [ ] #2 An in-memory extent store implements the interface, and Fxfs's and fxdax's tests pass on it
- [ ] #3 The deviation from upstream Fxfs is documented where it occurs, and scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
