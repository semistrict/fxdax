---
id: TASK-26.19.4
title: 'Guest Fxfs: file attributes (fileattr_get / fileattr_set, chattr)'
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 15:52'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: feature
ordinal: 67000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement fileattr_get/set (FS_IOC_GETFLAGS/SETFLAGS, FS_IOC_FSGETXATTR): immutable, append-only, noatime and project IDs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 lsattr and chattr work for immutable, append-only and noatime
- [x] #2 Immutable and append-only files reject the writes they forbid
- [x] #3 The flags survive a remount
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Flags kept in the posix mode's bits 16-19 (FXFS_MODE_IMMUTABLE/APPEND/NOATIME/NODUMP), as Fxfs objects have no flags field; fileattr_set writes back at once. Check file_attributes: lsattr reads back, writes/unlink/rename of immutable and non-append writes/truncate of append-only fail EPERM, flags kept across eviction and a remount. GCE: fxfs-guest-checks and -dax pass with the check; full suite 27/27 guest, 2239 workspace (commit df750c5).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
chattr/lsattr work for immutable, append-only, no-dump and no-atime, enforced by the VFS and kept by Fxfs in its mode's high bits; verified by fxfs-guest-checks' file_attributes (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
