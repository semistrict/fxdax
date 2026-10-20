---
id: TASK-26.19
title: 'Guest Fxfs: the VFS features ext4 and btrfs offer'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 11:55'
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
- [ ] #1 Every subtask is done or explicitly dropped with a reason
- [x] #2 Each feature not done (reflink, copy_file_range, fiemap, chattr, idmapped mounts, change counters, cache shrinking) is documented as unsupported, with what programs see instead
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 (commit 404a1e9): README 'Guest Fxfs: what it does not do' lists the features not done and what programs see; the guest checks' unsupported check pins three. TASK-26.19.3 fiemap was done in 67c5c64 (guest check sparse) but not finalized. AC1 left: 26.19.4, .6, .8, .10, .11 neither done nor dropped.
<!-- SECTION:NOTES:END -->
