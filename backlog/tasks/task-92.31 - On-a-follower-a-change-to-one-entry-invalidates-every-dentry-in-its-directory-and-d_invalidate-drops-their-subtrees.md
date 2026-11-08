---
id: TASK-92.31
title: >-
  On a follower, a change to one entry invalidates every dentry in its
  directory, and d_invalidate drops their subtrees
status: Done
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-08 04:51'
labels:
  - review-2026-10-21
  - wontfix
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 306000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
At dde8d58d3: Changes::add records only the directory for a child's keys (crates/fxfs/src/follower.rs:121-163, the Child arms at 147-153), drop_cached bumps its generation for each changed directory (guest/rust/fxfs-linux/src/follow.rs:144-166; fxfs_vfs_drop_entries, guest/linux/fs/fxfs/super.c:2999-3010), and d_revalidate fails older stamps without a re-lookup (fxfs_vfs_d_revalidate, super.c:357-366). One create, unlink or rename invalidates every cached child, and d_invalidate frees subdirectories' cached subtrees. Follower stat and walk loops pay µs re-lookups under the fill barrier instead of ~100 ns dcache hits, tens of ms per batch at 10k names, unlike the FUSE and NFS precedent its comment cites (super.c:357-361).

Where: crates/fxfs/src/follower.rs:121-163, guest/rust/fxfs-linux/src/follow.rs:144-166, guest/linux/fs/fxfs/super.c:357-366, 2999-3010.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. TASK-42.6 AC#2 specifies this generation design and is not a fix. Related: TASK-42.6, TASK-42.5.
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Frozen 2026-11-04 by the user: followers and the standalone store layer have no customer now; resumed after read-only DAX (TASK-164) and the storage collapse (TASK-165) land (docs/reviews/2026-11-04-first-principles.md, section 6).

2026-11-05 triage: citations brought to dde8d58d3; still frozen.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as won't fix, the user's decision of 2026-11-08 (OPEN_QUESTIONS.md, b0d7d00d2: 'Judgement calls, as recommended: close TASK-149.22, 44.1, 92.30 and 92.31 as won't fix'). A follower's dentries stay invalidated per directory generation (guest/linux/fs/fxfs/super.c:357-366, the design TASK-42.6 AC#2 specifies); per-entry invalidation would need Changes::add to record children's names through follower.rs, follow.rs and the guest's drop path, 1-2 days, for follower stat loops the user judged not worth it now. No code change.
<!-- SECTION:FINAL_SUMMARY:END -->
