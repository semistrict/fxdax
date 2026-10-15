---
id: TASK-26.19.8
title: >-
  Guest Fxfs: fs_flags and superblock flags (FS_MGTIME, FS_ALLOW_IDMAP,
  SB_I_VERSION)
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
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
- [ ] #1 Idmapped mounts of a Fxfs filesystem work
- [ ] #2 ctime changes are visible at fine granularity to stat after a change (FS_MGTIME)
- [ ] #3 statx reports a change attribute
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
