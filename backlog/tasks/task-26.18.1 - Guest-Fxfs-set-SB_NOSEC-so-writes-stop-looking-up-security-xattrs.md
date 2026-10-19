---
id: TASK-26.18.1
title: 'Guest Fxfs: set SB_NOSEC so writes stop looking up security xattrs'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-19 05:25'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: high
type: enhancement
ordinal: 55000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Filesystems on block devices get SB_NOSEC from get_tree_bdev(); btrfs sets it itself (fs/btrfs/super.c). Fxfs mounts without a block device, so every write() calls file_remove_privs(), which asks Fxfs for security.capability: a tree lookup per write (about 2% of mkdir-tree in a guest profile, fxfs_vfs_xattr_get under cap_inode_need_killpriv).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The guest superblock has SB_NOSEC set
- [x] #2 A guest profile of create or mkdir-tree shows no xattr lookup from the write path
- [x] #3 Setting security.capability or setuid on a file still makes the next write drop it (fxfs-guest-checks or pjdfstest)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. sb->s_flags |= SB_NOSEC in fxfs_vfs_fill_super (done in tree).
2. Build the x86 guest kernel; GCE profile of create/mkdir-tree shows no xattr lookup from writes.
3. fxfs-guest-checks-dax / pjdfstest suid-drop cases still pass (pjdfstest only at the end of the goal).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
S_NOSEC set on new inodes at create/tmpfile (a2a38cf): a new file's first write skips the security.capability lookup.

SB_NOSEC set at fill_super, S_NOSEC on new inodes (81cecd7). AC3 verified on GCE (29ca40b): write_removes_privileges: security.capability set, a write drops it; a set-user-ID file written by another user loses the bit. AC2 (a guest profile of create showing no xattr lookup on the write path) needs a bench run with FXDAX_BENCH_GUEST_PERF.

AC2 verified 2026-10-19: guest kernel profiled through every metadata and io workload (FXDAX_BENCH_GUEST_PERF, docs/measurements/gce-fs-bench-guest-perf-2026-10-19, c3-standard-8): create, mkdir-tree, create-fsync, create-parallel, rand-write, fsync-append and mmap-rand-write show no xattr, security.capability or killpriv frame (file_remove_privs, security_inode_need_killpriv, __vfs_getxattr); the one security_ frame is security_inode_permission, the lookup's permission hook.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
SB_NOSEC at fill_super and S_NOSEC on new inodes: writes no longer look up security xattrs, verified by the guest profile of every write workload; privileges are still dropped by a write (fxfs-guest-checks write_removes_privileges).
<!-- SECTION:FINAL_SUMMARY:END -->
