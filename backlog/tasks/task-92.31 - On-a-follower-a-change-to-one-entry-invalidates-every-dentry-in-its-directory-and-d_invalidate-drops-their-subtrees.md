---
id: TASK-92.31
title: >-
  On a follower, a change to one entry invalidates every dentry in its
  directory, and d_invalidate drops their subtrees
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 306000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Changes::add records only the directory (follower.rs:141-147), drop_cached bumps its generation (follow.rs:153-156; super.c:2305-2315), and d_revalidate fails older stamps without a re-lookup (281-285). One create, unlink or rename invalidates every cached child, and d_invalidate frees subdirectories' cached subtrees. Follower stat and walk loops pay µs re-lookups under the fill barrier instead of ~100 ns dcache hits, tens of ms per batch at 10k names, unlike the FUSE and NFS precedent cited (276-279).

Where: crates/fxfs/src/follower.rs:104-105, crates/fxfs/src/follower.rs:141-147, guest/rust/fxfs-linux/src/follow.rs:153-156, guest/linux/fs/fxfs/super.c:244-257, guest/linux/fs/fxfs/super.c:276-285, guest/linux/fs/fxfs/super.c:2305-2315.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. Changes::add records only the directory for Child keys (crates/fxfs/src/follower.rs:141-147). drop_cached calls fxfs_vfs_drop_entries per changed directory (guest/rust/fxfs-linux/src/follow.rs:153-156), which bumps the directory's gen (guest/linux/fs/fxfs/super.c:2307-2318). d_revalidate only compares d_time to gen and does no re-lookup (super.c:281-285). Not tracked. TASK-42.6 AC#2 specifies this generation design and is not a fix. Related: TASK-42.6, TASK-42.5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
