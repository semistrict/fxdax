---
id: TASK-26.3
title: 'Fxfs over an extent store, in userspace'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-13 19:54'
updated_date: '2026-10-13 22:20'
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
Fxfs's Device and allocator assume a fixed-size block device; the device size is read once at open (crates/fxfs/src/object_store/allocator.rs, device_size) and is both the bound on addresses and the capacity. In the design (doc-2) storage is the extent store of doc-4: a sparse address space per filesystem, storage used where written and released by FREE, so it grows and shrinks with the files. Doing this in userspace first, where Fxfs runs today, separates the storage change from the kernel port.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Fxfs runs on an extent store through storage_device's Device: addresses span the store's sparse address space, its capacity is the store's, not the address space, and space Fxfs frees is released to the store
- [ ] #2 An in-memory extent store implements the interface, and fxdax's tests, whose memory storage it backs, pass on it
- [ ] #3 A test crashes the in-memory store at random barriers while Fxfs writes, and Fxfs mounts and fscks every time
- [ ] #4 The deviation from upstream Fxfs is documented where it occurs, and scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/extent_store: doc-4's wire format (protocol), the ExtentStore trait, MemoryStore (sparse blocks, optional history of barrier epochs to simulate crashes), and ExtentDevice, a storage_device::Device over a store (PRE_BARRIER -> barrier write, flush -> COMMIT, close -> COMMIT_DURABLE, trim -> FREE).
2. Capacity: storage_device's Device gains capacity() (default: size), and Fxfs's allocator uses it, not device_size, for free-space accounting. Deviations documented in both files.
3. fxdax's Storage::memory backs onto MemoryStore (capacity = the size asked for); the full test suite passes.
4. Tests in extent_store: Fxfs on a 2^48-byte address space (format, workload, remount, fsck); storage released after trim; crash at random barriers then mount and fsck.
<!-- SECTION:PLAN:END -->
