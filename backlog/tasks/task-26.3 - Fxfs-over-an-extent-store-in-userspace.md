---
id: TASK-26.3
title: 'Fxfs over an extent store, in userspace'
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
updated_date: '2026-10-13 22:51'
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
Fxfs assumes a fixed-size block device: extent records hold device offsets, the allocator manages free space within the device's size, superblocks live at fixed offsets, and the journal and preallocated files are written in place. doc-4 replaces the device with an object store of write-once extents named by IDs the guest mints, plus a per-branch root record. The decisions are in doc-4's Decisions table: one on-disk format (extents only), whole-extent freeing by reference counts, branch+sequence IDs. Doing this in userspace first, where Fxfs runs today, separates the storage change from the kernel port. An earlier attempt over a sparse address space is parked on branch sparse-address-space (its capacity accounting and crash tests may be reused).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fxfs stores everything as write-once extents through an extent-store interface: extent records hold (extent ID, offset), the allocator mints IDs and counts references, nothing is written in place (superblocks go to new extents named by the root record, the journal writes new extents, overwrite mode is copy-on-write), and unreferenced extents are freed after commit
- [ ] #2 An in-memory extent store implements the interface, and Fxfs's own tests run on it in place of FakeDevice
- [ ] #3 A test crashes the in-memory store at random barriers while Fxfs writes, and Fxfs mounts and fscks every time
- [ ] #4 Free space is counted against the store's capacity, and space runs out with Fxfs's NoSpace error
- [ ] #5 The deviation from upstream Fxfs is documented where it occurs, and scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
