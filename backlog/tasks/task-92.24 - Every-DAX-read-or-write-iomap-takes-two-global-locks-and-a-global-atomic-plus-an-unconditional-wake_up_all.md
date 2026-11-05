---
id: TASK-92.24
title: >-
  A DAX write iomap takes its file's semaphore twice, the window's lock and
  global atomics
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-05 03:04'
labels:
  - review-2026-10-21
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
At 9f81f6018, after writable DAX was deleted (37c331a6c), what is left of this is the write path's.

The read side was fixed by 69f13ff2b (2026-10-25): fxfs_dax_iomap_read takes file->sem once on a cache hit and holds it through its iomap (guest/linux/fs/fxfs/dax.c:1798-1830); fxfs_dax_use_range takes dax->lock only when the range isn't already last on the LRU (dax.c:1544-1552); fxfs_dax_released wakes only when wq_has_sleeper (dax.c:924-930), and runs only when a range's last user goes (fxfs_dax_put_range, dax.c:1634-1638). The write path's dev->lock went with the gate: fxfs_device_store_begin and fxfs_device_store_end were deleted with writable DAX (37c331a6c).

Left, on every write iomap (fxfs_dax_iomap_write, dax.c:2496-2560): fxfs_dax_get_range takes file->sem shared to look the range up (dax.c:1570-1573) and fxfs_dax_map_missing takes it again, held until fxfs_dax_iomap_end (dax.c:1767-1790, 2669-2690); fxfs_dax_use_range may take dax->lock. For pages not dirty yet, fxfs_dax_dirty (dax.c:2421-2480) reserves in Fxfs and adds to the global atomics dax->dirty and dax->private (fxfs_dax_take_private, dax.c:2361); and for pages not private yet, it sends PRIVATE, a device round trip (fxfs_device_private, guest/linux/fs/fxfs/device.c:273), counted in dax->privates. With writers on several vCPUs, these cache lines bounce on each first write to a page; that case is unmeasured, as is the PRIVATE round trip's share of write(2).

Where: guest/linux/fs/fxfs/dax.c:1544-1552, 1560-1632, 1767-1790, 2361, 2421-2480, 2496-2560, 2669-2690; guest/linux/fs/fxfs/device.c:273.

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
