---
id: TASK-26.3
title: 'Fxfs over an extent store, in userspace'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-13 19:54'
updated_date: '2026-10-14 02:48'
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
- [x] #1 Fxfs stores everything as write-once extents through an extent-store interface: extent records hold (extent ID, offset), the allocator mints IDs and counts references, nothing is written in place (superblocks go to new extents named by the root record, the journal writes new extents, overwrite mode is copy-on-write), and unreferenced extents are freed after commit
- [x] #2 An in-memory extent store implements the interface, and Fxfs's own tests run on it in place of FakeDevice
- [x] #3 A test crashes the in-memory store at random barriers while Fxfs writes, and Fxfs mounts and fscks every time
- [x] #4 Free space is counted against the store's capacity, and space runs out with Fxfs's NoSpace error
- [x] #5 The deviation from upstream Fxfs is documented where it occurs, and scripts/upstream-changes.py --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
On branch object-store, keeping Fxfs's and fxdax's tests green at each phase; merged to main with TASK-26.10, since fxdax's images need the local store.
A. Types, behaviour unchanged: storage_device gains ExtentId (branch, sequence: 128 bits) and ExtentRange (extent, range within it). Device offsets and ranges throughout Fxfs become extent addresses and extent ranges; the whole device is one extent meanwhile. On-disk format 60, the earliest supported: types holding locations get V60 versions, with no conversion from older ones.
B. Real extents: Device becomes doc-4's extent store (read/write/free by extent, commit, root, info). The allocator mints an extent per allocation (branch from the store, sequence in AllocatorInfo), keeps its tree of allocated ranges with owners and counts, per extent, and frees an extent once its last range is deallocated and flushed. Free-space tracking (strategy, trimming) goes; reservations count against capacity. FakeDevice becomes a memory extent store, with crash simulation.
C. Nothing in place: superblocks are written to new extents that the root record names (A and B kept as two slots), each journal flush writes new extents, overwrite mode and preallocation become copy-on-write.
D. fxfs-platform's shared-block cache keyed by extent; tests: crashes at random barriers, capacity, release; deviations documented in each file.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Phases A-D landed on branch object-store (local, not on main until TASK-26.10, as planned): 4147a16 types; f450bbe device = extent store; a4e3d17 allocator mints/frees extents; 99e7551, 4d1150d FakeDevice write-once (always); d71483d super-blocks in new extents named by the root; 2642cd8 journal blocks are extents, tail renewed after replay, FreeAfter policy; 620fe00 overwrite mode copy-on-write; d5e1f72 FakeDevice records crashes at barriers; dcf5691 crash test (AC3) and fix: AllocatorInfoV60.dying, as a compaction otherwise leaked extents whose frees a crash undid; fa18873 capacity test (AC4); 850be8b deviation headers (AC5). Verified: scripts/test.sh --no-fail-fast, macOS 1843 run 1842 passed, Linux (Lima) 1847 run 1846 passed; the 1 failure is test_encrypted_mutations_mixed_versions, obsolete with format 60 and awaiting the user's OK to delete, which is why DoD #1 is unchecked. scripts/upstream-changes.py --check passes. Known limitation: fallocate no longer guarantees space (a flush of a preallocated range needs new blocks).
<!-- SECTION:NOTES:END -->
