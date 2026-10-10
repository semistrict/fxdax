---
id: TASK-21
title: 'Fxfs volume snapshots: clone a whole filesystem tree by sharing its extents'
status: To Do
assignee: []
created_date: '2026-10-10 14:05'
labels: []
dependencies:
  - TASK-20
type: feature
ordinal: 21000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Goal: cloning a whole filesystem (a volume, TASK-20) is nearly O(1) and takes almost no disk, by sharing the parent's metadata and data instead of copying them. Today the only clone is of the image file (Storage::clone_to / Filesystem::clone_storage_to), which shares host blocks but yields a separate Fxfs instance.

What Fxfs has: a volume is an object store whose metadata is an LSM tree; persisted layers are immutable objects (crates/fxfs/src/lsm_tree.rs, object_store.rs; object_store.rs notes layers are kept so we can support snapshots). Data writes are copy-on-write except for files with overwrite ranges (data_object_handle.rs, OverwriteOptions). What it lacks: the allocator records one owner per extent, AllocatorValue::Abs { count, owner_object_id } with count always 1 (crates/fxfs/src/object_store/allocator.rs), so no extent can belong to two volumes, and per-owner byte accounting and volume deletion assume exclusive ownership. Fxfs upstream has no snapshots.

Design to work out in the plan: a snapshot creates a new store that starts from the parent's layer set (layers referenced, not copied) after flushing it; extents become shared (reference counts in the allocator, or a shared owner with counts), freeing decrements; writes to shared blocks always copy-on-write, including overwrite-range files; deleting a volume frees only what it alone references; fsck verifies reference counts. The on-disk format changes need a new version and migration of existing images. Changes to Fxfs are deviations from upstream and are documented where they occur.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A volume can be snapshotted into a new volume while mounted, after its dirty data is written back, in time independent of its size
- [ ] #2 The snapshot and the original change independently: writes, truncates, deletes and renames in one never show in the other, shown by tests
- [ ] #3 Shared extents are freed only when no volume references them; space accounting per volume and for the image stays correct
- [ ] #4 Writes to shared blocks copy on write, including files with overwrite ranges
- [ ] #5 fsck checks extent reference counts and passes after snapshot, write, and delete sequences
- [ ] #6 Existing images open and are migrated to the new on-disk version
- [ ] #7 fxdax exposes snapshotting a volume
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 Deviations from upstream documented where they occur
<!-- DOD:END -->
