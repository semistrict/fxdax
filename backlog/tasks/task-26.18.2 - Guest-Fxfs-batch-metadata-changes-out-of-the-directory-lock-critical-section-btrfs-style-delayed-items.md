---
id: TASK-26.18.2
title: >-
  Guest Fxfs: batch metadata changes out of the directory-lock critical section
  (btrfs-style delayed items)
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
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
- [ ] #1 A design note (backlog doc) covers ordering, crash consistency, lookups that must see queued changes, and fsync
- [ ] #2 unlink-shared-dir and unlink-parallel are within 2x of ext4-dax-pmem on GCE
- [ ] #3 fxfs-guest-dax, fxfs-guest-checks-dax and fxfs-guest-crash-dax pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
