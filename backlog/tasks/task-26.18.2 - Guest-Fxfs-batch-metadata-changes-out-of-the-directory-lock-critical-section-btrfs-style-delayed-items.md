---
id: TASK-26.18.2
title: >-
  Guest Fxfs: batch metadata changes out of the directory-lock critical section
  (btrfs-style delayed items)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
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
- [ ] #2 unlink-shared-dir and unlink-parallel are within 2x of ext4-dax-pmem on GCE
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
<!-- SECTION:NOTES:END -->
