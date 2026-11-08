---
id: TASK-92.24
title: >-
  A DAX write iomap takes its file's semaphore twice, the window's lock and
  global atomics
status: Done
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-08 04:19'
labels:
  - review-2026-10-21
  - superseded
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 299000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
At dde8d58d3, after writable DAX was deleted (37c331a6c) and a range's page state went into pages.c (bda2e53a0), what is left of this is the write path's.

The read side was fixed by 69f13ff2b (2026-10-25): fxfs_dax_iomap_read takes file->sem once on a cache hit and holds it through its iomap (guest/linux/fs/fxfs/dax.c:1780-1874); fxfs_dax_use_range takes dax->lock only when the range isn't already last on the LRU (dax.c:1524-1532); fxfs_dax_released wakes only when wq_has_sleeper (dax.c:908-914), and runs only when a range's last user goes (fxfs_dax_put_range, dax.c:1614-1618). The write path's dev->lock went with the gate: fxfs_device_store_begin and fxfs_device_store_end were deleted with writable DAX (37c331a6c).

Left, on every write iomap (fxfs_dax_iomap_write, dax.c:2455-2517): fxfs_dax_get_range takes file->sem shared to look the range up (dax.c:1540-1612, the semaphore at 1550-1554, dax->lock at 1590) and fxfs_dax_map_missing takes it again (dax.c:1747-1775), held until fxfs_dax_iomap_end (dax.c:2626-2653, released at 2644); fxfs_dax_use_range may take dax->lock. For pages not dirty yet, fxfs_dax_dirty (dax.c:2384-2443) reserves in Fxfs and adds to the global atomics dax->dirty (2433) and dax->private (fxfs_dax_take_private, dax.c:2330-2343); and for pages not private yet, it sends PRIVATE, a device round trip (dax.c:2413; fxfs_device_private, guest/linux/fs/fxfs/device.c:273), counted in dax->privates (2415). With writers on several vCPUs, these cache lines bounce on each first write to a page; that case is unmeasured, as is the PRIVATE round trip's share of write(2).

Where: guest/linux/fs/fxfs/dax.c:908-914, 1524-1532, 1540-1612, 1747-1775, 2330-2343, 2384-2443, 2455-2517, 2626-2653; guest/linux/fs/fxfs/device.c:273.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. Related: TASK-79.12, TASK-26.18, TASK-26.15.
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
2026-11-05 triage: citations brought to dde8d58d3 (dax.c moved by f22c9459e, bda2e53a0 and 311f7b08b).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Superseded by later measurement: what this guards against, a DAX write(2) slowed by its iomap's semaphore, the window's lock, global atomics and its PRIVATE round trip, no longer holds DAX back. TASK-164.11 moved a write's PRIVATE before the inode's lock (r807: lock waits 37.5 s to 7.4 s, forked-dax 0.912). The full perf gate on main (r903, docs/measurements/dax-cow-gate.md) has DAX pgbench at 0.977 of no DAX and forked-dax 0.949, every gate passing, so the double semaphore and the atomics cost nothing the gate can see. Closed under TASK-92's AC (a measurement showing it costs nothing worth fixing); the review's cache-line bouncing with writers on many vCPUs stays unmeasured.
<!-- SECTION:FINAL_SUMMARY:END -->
