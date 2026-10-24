---
id: TASK-79.97
title: 'Guest Fxfs: buffered fallback inodes on DAX followers keep stale cached data'
status: To Do
assignee: []
created_date: '2026-10-24 00:54'
labels:
  - review-2026-10-23
dependencies: []
references:
  - guest/linux/fs/fxfs/super.c
  - guest/linux/fs/fxfs/dax.c
  - guest/tests/test_cases/src/test_fxfs_guest_follow.rs
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 367000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the 2026-10-23 adversarial review at bce42647. Severity P2; high static confidence, not dynamically reproduced. fxfs_dax_attach can fail kzalloc or xa_store; fxfs_dax_set_inode deliberately leaves that inode buffered without S_DAX (guest/linux/fs/fxfs/dax.c:2425-2463). fxfs_vfs_drop_data instead dispatches from the mount-wide dax pointer and returns after fxfs_dax_changed (super.c:2322-2345), which immediately succeeds without invalidating non-DAX inodes (dax.c:2510-2513). Writer overwrites can advance follower settled epochs and attributes while reads and mappings retain old cached bytes until unrelated eviction. Scope: per-inode data invalidation while preserving inode-less cached DAX range invalidation. Related TASK-42 and TASK-42.6. Existing tests omit the mixed DAX-mount and buffered-inode fallback. Estimated fix effort small to medium including fault injection; fix risk low.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Changed data ranges invalidate the cache appropriate to each inode actual mapping mode, including buffered inodes on mounts with DAX enabled.
- [ ] #2 A regression forces DAX attachment allocation failure, caches old follower data, applies a writer overwrite and waits for the settled epoch; subsequent reads and shared mappings must see new bytes. It fails before the fix.
- [ ] #3 Ordinary DAX inodes and inode-less cached DAX ranges continue to invalidate correctly.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 The fix batch passes cargo build or cargo check, cargo clippy, scripts/test.sh on macOS and scripts/test-gce.sh for the Linux workspace and applicable guest cases, with a rebuilt guest kernel for guest changes.
- [ ] #2 Changed upstream deviations are documented, scripts/upstream-changes.py --check passes, and generated files are updated through their generators.
<!-- DOD:END -->
