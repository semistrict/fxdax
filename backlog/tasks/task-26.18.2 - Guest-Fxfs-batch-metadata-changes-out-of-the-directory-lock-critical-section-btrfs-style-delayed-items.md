---
id: TASK-26.18.2
title: >-
  Guest Fxfs: batch metadata changes out of the directory-lock critical section
  (btrfs-style delayed items)
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 11:05'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: high
type: feature
ordinal: 56000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
unlink-shared-dir (about 3-5x ext4) and unlink-parallel (about 3x) are bounded by work done under the VFS directory i_rwsem: each Fxfs unlink builds a transaction, applies about 4 mutations to the in-memory skip list, updates the TreeCache and serializes a journal record before returning, while waiters spin on the rwsem (rwsem_spin_on_owner ~20% of guest CPU). btrfs, also a copy-on-write tree filesystem, queues directory-index and inode-item changes in memory (fs/btrfs/delayed-inode.c) and applies them to its B-tree in batches from workers. Fxfs could do the same: keep crash consistency through its journal, but take the per-op tree work and the graveyard round trip out of the syscall.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A design note (backlog doc) covers ordering, crash consistency, lookups that must see queued changes, and fsync
- [x] #2 unlink-shared-dir and unlink-parallel are within 2x of ext4-dax-pmem on GCE
- [x] #3 fxfs-guest-dax, fxfs-guest-checks-dax and fxfs-guest-crash-dax pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. doc-6 design. 2. Unlinks of non-directories queued in the guest's Rust, applied in batches of 32 in one transaction, drained before reads/changes of what they touch and before syncs; evict queued behind. 3. Guest checks of the semantics. 4. GCE bench.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
61424e2 (libkrun 15a0dd3). GCE: unlink 131-167k/s (ext4 208k), unlink-shared-dir 90-92k (168k, 1.85x), large-dir-unlink 70-75k (128k), unlink-parallel 80-94k (218k, still 2.3-2.7x: AC2 open).

111d515: unlink batches tombstone evicted inodes themselves (no graveyard round trip), appliers don't wait on each other. GCE: rm-tree 53-61k/s (ext4 62k), large-dir-unlink 88-104k (128k), unlink-parallel 96-98k (218k, still ~2.2x), unlink 150-160k (208k).

6d2e95c (background worker), 09d3e26 (names removed by key). GCE 3 iterations (eb7e4cb): unlink-shared-dir 1.80x, unlink-parallel 1.82x, unlink 1.09x of ext4-dax-pmem. Workspace 1871/1871, every guest case and pjdfstest pass at 09d3e26.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Unlinks of non-directories are queued and applied in batches of 32 by a background worker, one transaction each, tombstoning evicted inodes in the same transaction and removing names by key; reads and syncs drain the queue first. Verified by guest checks (delayed_unlinks), pjdfstest, crash tests and GCE benchmarks (unlink family 1.09-1.82x of ext4).
<!-- SECTION:FINAL_SUMMARY:END -->
