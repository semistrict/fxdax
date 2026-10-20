---
id: TASK-26.19
title: 'Guest Fxfs: the VFS features ext4 and btrfs offer'
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 19:22'
labels: []
dependencies: []
parent_task_id: TASK-26
priority: medium
type: feature
ordinal: 63000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A comparison with ext4 and btrfs in Linux 7.3 (fs/ext4, fs/btrfs) shows the guest Fxfs (guest/linux/fs/fxfs) implements the core inode, file and super operations only. Programs that use O_TMPFILE, fallocate, chattr, ACLs, reflinks, freezing or NFS export fail or fall back on it. Each subtask is one feature, with tests in the guest suites.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every subtask is done or explicitly dropped with a reason
- [x] #2 Each feature not done (reflink, copy_file_range, fiemap, chattr, idmapped mounts, change counters, cache shrinking) is documented as unsupported, with what programs see instead
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 (commit 404a1e9): README 'Guest Fxfs: what it does not do' lists the features not done and what programs see; the guest checks' unsupported check pins three. TASK-26.19.3 fiemap was done in 67c5c64 (guest check sparse) but not finalized. AC1 left: 26.19.4, .6, .8, .10, .11 neither done nor dropped.

2026-10-20 (df750c5): .4, .8, .10, .11 done (.8 AC3 and .11 AC3 not met, reasons in their notes and README); .3 finalized. Left for AC1: .6 reflinks, which need reference-counted ranges in Fxfs's allocator and copy-on-write through the DAX write path.

AC1 met: every subtask done (26.19.6 reflinks in 6fa0684/bb41664); .8 AC3 and .11 AC3 not met, with reasons in their notes and the README.

Final: bb41664 passes 27/27 guest tests (pjdfstest included) and 2243 Linux workspace tests on GCE, and the macOS suite (2176).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The guest Fxfs has O_TMPFILE, fallocate, fiemap, chattr flags, POSIX ACLs, reflinks, freezing, show_options, NFS file handles, idmapped mounts, multigrain times, a cache shrinker, O_DIRECT and large folios; what it lacks (dedupe, i_version, MAP_SYNC) is in the README.
<!-- SECTION:FINAL_SUMMARY:END -->
