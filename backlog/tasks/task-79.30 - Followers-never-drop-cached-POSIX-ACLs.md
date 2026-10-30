---
id: TASK-79.30
title: Followers never drop cached POSIX ACLs
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-30 10:18'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 218000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Settling an inode on a follower refreshes mode, owner, size, times and nlink, but never calls forget_all_cached_acls. ACL removals made on the writer (setfacl -x or -m u:bob:---) do not take effect on followers. This is within open TASK-42.6 AC#2's attribute refresh, which does not name ACLs.

Where: guest/linux/fs/fxfs/super.c:347, guest/linux/fs/fxfs/super.c:2226, guest/linux/fs/fxfs/super.c:2285.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.6, TASK-26.19.5. Not tracked as its own task. It falls inside TASK-42.6 AC#2 (In Progress: attributes refreshed after a batch), which does not name ACLs.
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
fxfs_vfs_settle_attr drops a settled inode's cached ACLs. fxfs-guest-follow, -dax check a writer's ACL change is followed: r284 both pass; r282 without the fix the follower never reports seen (its console cut off mid-panic).
<!-- SECTION:NOTES:END -->
