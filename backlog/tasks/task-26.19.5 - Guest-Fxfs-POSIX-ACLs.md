---
id: TASK-26.19.5
title: 'Guest Fxfs: POSIX ACLs'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-15 08:56'
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
- [x] #2 Default ACLs on directories apply to new children
- [ ] #3 pjdfstest still passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
81cecd7: SB_POSIXACL, get_inode_acl/set_acl over xattrs as btrfs, posix_acl_create in every create. Guest check acls passes (set/get, mode from mask, default ACL inherited, umask without one). Permission enforcement for other users and pjdfstest not yet verified.
<!-- SECTION:NOTES:END -->
