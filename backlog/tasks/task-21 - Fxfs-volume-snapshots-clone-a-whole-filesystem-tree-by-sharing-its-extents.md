---
id: TASK-21
title: 'Fxfs volume snapshots: clone a whole filesystem tree by sharing its extents'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 14:05'
updated_date: '2026-10-10 15:37'
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
- [x] #1 A volume can be snapshotted into a new volume while mounted, after its dirty data is written back, in time independent of its size
- [x] #2 The snapshot and the original change independently: writes, truncates, deletes and renames in one never show in the other, shown by tests
- [x] #3 Shared extents are freed only when no volume references them; space accounting per volume and for the image stays correct
- [x] #4 Writes to shared blocks copy on write, including files with overwrite ranges
- [x] #5 fsck checks extent reference counts and passes after snapshot, write, and delete sequences
- [x] #6 Existing images open and are migrated to the new on-disk version
- [x] #7 fxdax exposes snapshotting a volume
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #5 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design: generations of data ownership inside the allocator; allocator records keep their format.
- Each store has a current data owner (its own id unless a snapshot changed it). Allocations record the resolved owner, and accounting (owner_bytes) follows record owners; reservations and limits stay keyed by store id and resolve to the current owner.
- A snapshot of store V into new store S freezes V's current owner F: V gets a fresh owner G; S's own owner is its id; both list F (and V's earlier bases) as bases; frozen owners count the stores referencing them. In-memory reserved/uncommitted counters move from F to G.
- Deallocating a range owned by a frozen owner does nothing (shared); only parts owned by the store's current owner are freed. When a volume is deleted, its current owner is marked for deletion and each base's count drops; a base reaching 0 is marked for deletion. Space a snapshot shares is reclaimed once no volume references its base.
- Metadata: S starts from V's persistent layers, after V is flushed (its writes after the flush point are V's alone). Layer files are shared by reference count (root store adjust_refs); compaction releases old layers with adjust_refs and tombstones only those reaching 0.
- Overwrite-mode files: multi_overwrite moves any range it would write in place that a frozen owner holds to newly allocated space (same extent mode), so shared blocks are never overwritten.
- fsck: a store's extents must be owned by its current owner (checked exactly, as now) or by one of its bases (shared: any number of stores may reference them, or none); owners may be stores, generations or frozen owners.
- On-disk format 59: AllocatorMutation gains Snapshot { source, new_store, new_owner }; AllocatorInfo gains generations, frozen counts and bases; Mutation and JournalRecord versions follow; older versions migrate.
Steps:
1. Versioned types (AllocatorMutationV59, MutationV59, JournalRecordV59, AllocatorInfoV59, LATEST_VERSION 59), fingerprints.
2. Allocator: owner resolution, Snapshot and MarkForDeletion semantics, allocate/mark_allocated/deallocate, accounting moves, is_frozen lookups.
3. Layer sharing in flush; RootVolume::snapshot_volume (flush source, new store from its layers and StoreInfo, layer refs +1, allocator Snapshot mutation, volume directory entry).
4. multi_overwrite copy-on-write of frozen ranges.
5. fsck.
6. VolumesDirectory and fxdax: Image::snapshot_volume(source, name) for a mounted or unmounted volume, after writing back the source's dirty data.
7. Tests: fxfs unit tests (snapshot independence, deletion order frees space, overwrite files, fsck, remount/replay); fxdax image tests; existing images open (migration).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented as planned. Snapshot = RootVolume::snapshot_volume: takes the source's flush lock, flushes it, copies its StoreInfo (new guid, in-memory last object id) into a new store whose layers it shares (root store adjust_refs +1, layers locked), adds an allocator Snapshot mutation (new owner id from the root store's object ids; replay advances its last object id) and the volume directory entry, then opens and registers the store. Allocator: AllocatorInfoV59 (data_owners, frozen_owners, shared_owners); allocations are recorded and counted under a store's data owner; Snapshot freezes the source's owner (shared by both, plus the source's earlier frozen owners), moves its reserved/uncommitted counters to the new owner; deallocate skips ranges frozen owners hold (a merger query, only when frozen owners exist); MarkForDeletion releases a volume's frozen owners, deleting each at 0. Compaction and volume deletion release layers by refcount (and lock them). multi_overwrite writes shared ranges to new blocks (overwrite_shared: fresh reservation moved into the transaction's, extents recorded fully written); DataObjectHandle::overwrite refuses shared raw extents. fsck: shared ranges collected from the allocator; scanner records unshared extents under the store's data owner; verify_allocations skips frozen owners' records and accepts generation/frozen owners; shared layers count one root reference per listing store; zombie check only for non-root parents. Format 59; AllocatorMutation's Snapshot variant appended (bincode compatible). fxdax: Image::snapshot_volume (writes back the source's dirty pages if mounted; EEXIST/ENOENT), VolumesDirectory::snapshot_volume. Space a snapshot shares is reclaimed only when no volume references the frozen owner (documented). Validation: fxfs tests test_snapshot_volume, _replays (crash replay via device snapshot), _deletion_frees_space (both orders, back to baseline on disk), _overwrite_extents, _chain, allocator_info_migrates_from_version_32; fxdax tests snapshot_a_mounted_volume, snapshot_a_preallocated_file (pager writeback through overwrite_shared, confirmed by a temporary trace); suites macOS 1826, Linux 1829; guest tests pass on both.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Volume snapshots in Fxfs: a whole volume clones in time and space independent of its size by sharing its layer files (reference counted) and data extents (frozen allocation owners inside the allocator; allocator records keep their format; on-disk version 59 adds AllocatorInfoV59). Snapshot and source change independently; preallocated files write shared blocks to new ones; deleting volumes in any order frees shared space once unreferenced; fsck understands shared extents; old images migrate. Exposed as fxdax::Image::snapshot_volume. Verified by new fxfs and fxdax tests (including crash replay and space reclamation), the suites on macOS and Linux, and the guest tests.
<!-- SECTION:FINAL_SUMMARY:END -->
