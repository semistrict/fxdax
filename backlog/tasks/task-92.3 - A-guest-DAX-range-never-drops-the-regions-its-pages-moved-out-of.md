---
id: TASK-92.3
title: A guest DAX range never drops the regions its pages moved out of
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 12:48'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 278000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
keep_region only appends (dax.c:1059-1083), relocate drops nothing (333-352), and region references are put only in free_range (684-685). So a WAL page rewritten every commit pins at least 64 KiB of window per seal (963-965, 1945-1949). pgbench-tps runs about 730 seals/s with 0.00 UNMAPs per op, so the 2 GiB window fills in about 35-45 s, or in any long-lived DB VM. After that, restage and move fail with -ENOSPC (1107, 1998-2000), each post-seal WAL write takes a synchronous MAP inside the gate (426-431), and reclaiming a WAL range sends one serial UNMAP per region (641-643), thousands in one syscall.

Where: guest/linux/fs/fxfs/dax.c:1059-1083, guest/linux/fs/fxfs/dax.c:333-352, guest/linux/fs/fxfs/dax.c:684-685, guest/linux/fs/fxfs/dax.c:963-965, guest/linux/fs/fxfs/dax.c:1945-1949, guest/linux/fs/fxfs/dax.c:1107, guest/linux/fs/fxfs/dax.c:1998-2000, guest/linux/fs/fxfs/dax.c:426-431, guest/linux/fs/fxfs/dax.c:641-643.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. dax.c has changed only in 3a6ca6d (2 MiB faults) since the review. fxfs_dax_keep_region only appends (guest/linux/fs/fxfs/dax.c:1061-1085). fxfs_dax_relocate drops nothing (333-352). A range puts its regions only in fxfs_dax_free_range (675-689). The window allocator still returns -ENOSPC (559-571). No task tracks it. TASK-26.16's notes leave window fragmentation and region-aware reclaim to TODO.md. Related: TASK-26.16, TASK-26.18.9, TASK-91.4, TASK-91.9.
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
Fixed by TASK-91.10's work: ranges let go of regions their pages left (009ae677, fxfs_dax_prune), and after each seal the pages ranges moved into the staging extents' regions move to the ranges' own space and the emptied regions go (cb43345f, fxfs_dax_evacuate_work; f859553d1 for evict/truncate). Evidence: fxfs-guest-checks-dax regions_pruned (400 fsynced rewrites: 399 regions more before, <= 32 after) and regions_evacuated (400 single writes: 400 more before, <= 32 after); DAX pgbench on GCE: regions 360-540 (was 1,150-1,750, or 5,400-20,000 before pruning), the regions' pool full 0-1 times a run (was 75-99k), range MAPs 66-143 a run (was 54-72k).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A DAX range now drops the regions its pages leave (pruning, 009ae677) and after each seal its pages leave the staging regions (evacuation, cb43345f): the window no longer fills with regions; verified by regions_pruned and regions_evacuated (red then green) and DAX pgbench's counters on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
