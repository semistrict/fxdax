---
id: TASK-79.29
title: Inherited ACLs are written in a later transaction than the inode's creation
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 217000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The new inode is committed with its ACL-masked mode, while the inherited ACLs wait in memory until write_inode. Any fsync makes the creation durable without them, so after a crash the file grants its group the mask's permissions, and a new directory loses its default ACL.

Where: guest/linux/fs/fxfs/super.c:490, guest/linux/fs/fxfs/super.c:507, guest/linux/fs/fxfs/super.c:407, guest/linux/fs/fxfs/super.c:541.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.5. Not tracked. TASK-26.19.5 (Done) added posix_acl_create in every create but writes inherited ACLs only at write_inode.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
