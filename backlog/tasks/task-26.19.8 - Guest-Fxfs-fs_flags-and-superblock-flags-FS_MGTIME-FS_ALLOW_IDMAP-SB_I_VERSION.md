---
id: TASK-26.19.8
title: >-
  Guest Fxfs: fs_flags and superblock flags (FS_MGTIME, FS_ALLOW_IDMAP,
  SB_I_VERSION)
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 15:52'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: feature
ordinal: 71000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The Fxfs file_system_type sets no fs_flags. ext4 sets FS_ALLOW_IDMAP (idmapped mounts, used by container runtimes) and FS_MGTIME (multigrain timestamps); btrfs and ext4 set SB_I_VERSION for change attributes (NFS, IMA).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Idmapped mounts of a Fxfs filesystem work
- [x] #2 ctime changes are visible at fine granularity to stat after a change (FS_MGTIME)
- [ ] #3 statx reports a change attribute
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
FS_ALLOW_IDMAP and FS_MGTIME set; new inodes' owners via mapped_fsuid/mapped_fsgid. Checks idmapped_mount (0-999 -> 10000-10999: root's file shows 10000, 10005's create is 5 on disk, root gets EOVERFLOW) and multigrain_times (200 back-to-back chmods, each ctime strictly later). AC3 (statx change attribute) not met: STATX_CHANGE_COOKIE is never reported to user space, and SB_I_VERSION needs an on-disk counter Fxfs lacks; with FS_MGTIME, nfsd uses the fine-grained ctime as the change attribute. Documented in README. GCE: fxfs-guest-checks and -dax pass with the check; full suite 27/27 guest, 2239 workspace (commit df750c5).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Idmapped mounts and multigrain timestamps work (idmapped_mount, multigrain_times checks, GCE); no i_version: the fine-grained ctime is the change attribute, as README says.
<!-- SECTION:FINAL_SUMMARY:END -->
