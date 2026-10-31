---
id: TASK-79.97
title: 'Guest Fxfs: buffered fallback inodes on DAX followers keep stale cached data'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-24 00:54'
updated_date: '2026-10-31 02:23'
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
- [x] #1 Changed data ranges invalidate the cache appropriate to each inode actual mapping mode, including buffered inodes on mounts with DAX enabled.
- [x] #2 A regression forces DAX attachment allocation failure, caches old follower data, applies a writer overwrite and waits for the settled epoch; subsequent reads and shared mappings must see new bytes. It fails before the fix.
- [x] #3 Ordinary DAX inodes and inode-less cached DAX ranges continue to invalidate correctly.
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 The fix batch passes cargo build or cargo check, cargo clippy, scripts/test.sh on macOS and scripts/test-gce.sh for the Linux workspace and applicable guest cases, with a rebuilt guest kernel for guest changes.
- [x] #2 Changed upstream deviations are documented, scripts/upstream-changes.py --check passes, and generated files are updated through their generators.
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. super.c: drop_data dispatches on the inode's own mode, not the mount's; fxfs.dax_fail_attach knob; fxfs-guest-follow-dax extended, NOT YET RUN.

Mutant GCE r297, drop_data on the mount's mode: fxfs-guest-follow-dax fails, the follower doesn't see its writer's changes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
drop_data goes by the inode's mode, not the mount's. fxfs-guest-follow[-dax] with fxfs.dax_fail_attach forcing a fallback inode, r293 3 rounds each; on the mount's mode the follower never sees its writer's change (r297). DAX inodes' invalidation unchanged, follow-dax mappings checked. Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
