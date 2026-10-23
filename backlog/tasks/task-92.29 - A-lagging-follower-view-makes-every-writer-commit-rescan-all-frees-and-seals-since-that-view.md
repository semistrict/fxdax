---
id: TASK-92.29
title: >-
  A lagging follower view makes every writer commit rescan all frees and seals
  since that view
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 304000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each commit, under the state lock, walks the view-kept frees again in trim (kept.rs:86-91), every free since the oldest view in views_bytes (105-121), and every seal since then in sealed.retain (store.rs:1851-1855). fxdax sets no view_lag or view_kept_bytes (extents.rs:612, 634; store.rs:87-88). At a measured lag of 881 commits that is about 100 µs per commit against an 11 µs store phase, and it grows without limit for a paused follower or a point-in-time mount.

Where: crates/extent_store/src/store.rs:87-88, crates/extent_store/src/store.rs:1675-1680, crates/extent_store/src/store.rs:1837-1855, crates/extent_store/src/kept.rs:86-91, crates/extent_store/src/kept.rs:105-121, crates/fxdax/src/extents.rs:612, crates/fxdax/src/extents.rs:634.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present after the State split. Each publish, under the state lock (store.rs:1033-1053), runs KeptExtents::trim over released..=published, leaving view-kept entries to be rescanned (kept.rs:93-110). It also runs views_bytes over every free after the oldest view, possibly twice (kept.rs:114-128), and Sealed::published runs retain over every seal since the oldest view (views.rs:268-276). fxdax still opens stores with Options::default() (crates/fxdax/src/host_store.rs:42-44), whose view_lag and view_kept_bytes are None (store.rs:86-97). Not tracked. Related: TASK-42.2, TASK-42.7, TASK-79.24.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A commit now costs only what a lagging view lets go of. Trim scans frees up to the oldest view's commit only, the bytes views keep are counted on as the oldest view moves, and seals are kept in seal order and dropped from the front. In store-bench lagging-view, commits with a view that never advances took 50-394 us each as it fell 20,000 commits behind; they now take 0.5 us, as with no view. A paused follower or a point-in-time mount no longer slows writer commits. It still keeps its frees as used space, because fxdax sets no view_lag or view_kept_bytes. The macOS and Linux (GCE) suites pass.
<!-- SECTION:FINAL_SUMMARY:END -->
