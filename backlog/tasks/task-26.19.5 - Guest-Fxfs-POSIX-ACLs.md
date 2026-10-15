---
id: TASK-26.19.5
title: 'Guest Fxfs: POSIX ACLs'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: medium
type: feature
ordinal: 68000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement get_inode_acl/set_acl and mount with SB_POSIXACL; Fxfs stores extended attributes but the kernel does not treat system.posix_acl_* as ACLs on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 setfacl/getfacl work and permission checks honour ACLs
- [ ] #2 Default ACLs on directories apply to new children
- [ ] #3 pjdfstest still passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
