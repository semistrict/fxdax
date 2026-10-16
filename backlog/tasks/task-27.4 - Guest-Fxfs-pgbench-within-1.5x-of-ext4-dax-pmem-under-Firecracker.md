---
id: TASK-27.4
title: 'Guest Fxfs: pgbench within 1.5x of ext4-dax-pmem under Firecracker'
status: Done
assignee: []
created_date: '2026-10-16 17:18'
updated_date: '2026-10-16 17:25'
labels: []
dependencies: []
parent_task_id: TASK-27
ordinal: 79000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
pgbench (PostgreSQL 17, scale 1, one client, 20 s) on the guest kernel's Fxfs with a DAX window ran 2.1-2.2x slower than ext4 on DAX pmem under Firecracker on GCE; PostgreSQL's initdb also failed now and then with ENOSPC/EIO as the journal's metadata reservation drained.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 pgbench-tps of fxfs-guest-dax is under 1.5x of ext4-dax-pmem in a 3-iteration GCE run, recorded in docs/measurements
- [x] #2 initdb succeeds in every iteration, with no journal or unlink errors in the guest's log
- [x] #3 Guest suites (extent-guest, fxfs-guest, -dax, -checks, -checks-dax, -crash-dax), pjdfstest-fxfs-guest-dax and the workspace tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Linux workspace tests ran on GCE (fxdax-test-t2, 1884/1884), not Lima (GCE-only per user). scripts/upstream-changes.py --check passes; fxfs deviations carry fxdax: comments.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
pgbench 2.1-2.2x -> 1.34x (gce-fs-bench-2026-10-16T1653Z: 1220 vs 1637 tps; 1.36-1.43x in four more runs). Extent files written through the page cache (c065b46); fsync no longer waits for FREEs (3993610); freed extents' window mappings map zeros, indexed by what keeps them (ac6d143); borrowing transactions reserve first, ending the initdb ENOSPC/EIO (2c1da57, 569fb5e); a read-only MAP past an extent's end maps zeros, fixing seal_race's intermittent EINVAL (695ee33, doc-4 updated). Verified: workspace 1878/1878; guest suites and pjdfstest-fxfs-guest-dax on GCE; seal_race 10/10 runs; 15 initdbs clean across five runs.
<!-- SECTION:FINAL_SUMMARY:END -->
