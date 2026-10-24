---
id: TASK-79.96
title: 'Guest Fxfs: chmod leaves named-user ACL permissions unchanged'
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-24 00:53'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-23
dependencies: []
references:
  - guest/linux/fs/fxfs/super.c
  - guest/tests/test_cases/src/test_fxfs_guest_checks.rs
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 366000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-10-23 adversarial review at bce42647. Severity P1; high confidence from source inspection, not dynamically reproduced. fxfs_vfs_setattr changes inode mode without updating the access ACL for ATTR_MODE (guest/linux/fs/fxfs/super.c:722-741). A named user granted rw through an extended ACL can still newly open for writing after mode changes from 0660 to 0640: Linux still consults the unchanged ACL mask when group mode bits remain nonzero. Existing ACL tests cover initial authorization and inheritance but omit chmod revocation (guest/tests/test_cases/src/test_fxfs_guest_checks.rs:788-848). Scope: effective, cached and persisted access ACL permissions and mode/ACL failure consistency. Related TASK-26.19.5; distinct from TASK-79.29 creation ordering and TASK-79.30 follower ACL invalidation. Estimated fix effort small; fix risk medium.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Changing mode updates effective access ACL permissions consistently with the requested mode while retaining named entries.
- [x] #2 A regression fails before the fix: a named user loses write-open access after chmod reduces the ACL group-class mask from rw to r; denial remains after fsync, inode reload and remount.
- [x] #3 ACL-update failures are reported and leave a consistent permission state.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 The fix batch passes cargo build or cargo check, cargo clippy, scripts/test.sh on macOS and scripts/test-gce.sh for the Linux workspace and applicable guest cases, with a rebuilt guest kernel for guest changes.
- [ ] #2 Changed upstream deviations are documented, scripts/upstream-changes.py --check passes, and generated files are updated through their generators.
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
acls' chmod revocation failed on the kernel before (vmlinux-evac2, with and without DAX) and passes with 075723b80 (posix_acl_chmod in setattr). GCE r65, 2026-10-24, batch on main+round 2b: Linux workspace 2620 passed, Firecracker extent tests, and 17 guest cases (checks, crash, fxfs-guest, follow, follow-torture, snapshot with and without DAX; extent-guest, live-fork, daemon-lost, pgbench both) green on vmlinux-b3/b4; macOS scripts/test.sh 2508 passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A chmod now updates the access ACL's mask, so it revokes what it takes away from a named user; the acls check red then green on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
