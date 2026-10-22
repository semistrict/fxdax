---
id: TASK-92.24
title: >-
  Every DAX read or write iomap takes two global locks and a global atomic, plus
  an unconditional wake_up_all
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
ordinal: 299000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each iomap takes file->sem twice (dax.c:1273-1277, 1467-1492), the global dax->lock for an LRU move (1325-1327), and nearly always a global atomic plus wake_up_all with no waiter check (1331-1335, 543-547); writes add the global dev->lock 2-4 times. Single-threaded, this is about 30-60 ns of rand-read-warm's 1.26 µs. With parallel readers on several vCPUs, these cache lines bounce on every I/O; that case is unmeasured.

Where: guest/linux/fs/fxfs/dax.c:543-547, guest/linux/fs/fxfs/dax.c:1273-1277, guest/linux/fs/fxfs/dax.c:1325-1327, guest/linux/fs/fxfs/dax.c:1331-1335, guest/linux/fs/fxfs/dax.c:1467-1492, guest/linux/fs/fxfs/dax.c:1573, guest/linux/fs/fxfs/dax.c:1704, guest/linux/fs/fxfs/device.c:141-193.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present; guest dax.c changed only for PMD faults since 6a0a3f8. fxfs_dax_get_range takes file->sem with down_read (guest/linux/fs/fxfs/dax.c:1275) and always takes dax->lock for list_move_tail (1327-1329). The read iomap takes file->sem again (1469). put_range then runs fxfs_dax_released, an atomic_long_inc plus an unconditional wake_up_all with no wq_has_sleeper check (544-549, 1332-1336). Writes take dev->lock in store_begin, store_end and seals (device.c:140-195). Not tracked. Related: TASK-79.12, TASK-26.18, TASK-26.15.
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
