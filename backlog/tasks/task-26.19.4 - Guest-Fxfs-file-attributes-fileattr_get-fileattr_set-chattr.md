---
id: TASK-26.19.4
title: 'Guest Fxfs: file attributes (fileattr_get / fileattr_set, chattr)'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
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
- [ ] #1 lsattr and chattr work for immutable, append-only and noatime
- [ ] #2 Immutable and append-only files reject the writes they forbid
- [ ] #3 The flags survive a remount
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
