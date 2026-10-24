---
id: TASK-92.28
title: >-
  Every write that extends the file runs a Fxfs transaction, even into blocks
  already writable
status: In Progress
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 21:20'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 303000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fast path requires size <= i_size (dax.c:1579-1581), so every extending write kvmallocs 28 KiB (246-250, 1592-1593), runs a full Fxfs transaction in map_write (fs.rs:797-815), and takes invalidate_lock exclusively (1791-1797), even when nothing is allocated. Sub-page log appends without per-write fsync cost about 10 µs each against 1-2 µs on ext4-dax, and journal flushes come sooner.

Where: guest/linux/fs/fxfs/dax.c:246-250, guest/linux/fs/fxfs/dax.c:1579-1581, guest/linux/fs/fxfs/dax.c:1592-1593, guest/linux/fs/fxfs/dax.c:1610-1612, guest/linux/fs/fxfs/dax.c:1791-1797, guest/rust/fxfs-linux/src/fs.rs:797-815, crates/fxfs/src/object_store/data_object_handle.rs:1486-1520.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. The write fast path still requires size <= i_size_read (guest/linux/fs/fxfs/dax.c:1576-1580). Otherwise it kvmallocs FXFS_DAX_PAGES extents (246-250, 1592-1597) and calls fxfs_dax_map_write (1611-1614), which runs a full Fxfs transaction (txn_map_write plus txn_update_size, guest/rust/fxfs-linux/src/fs.rs:795-815). write_iter takes invalidate_lock exclusively for any extending write (dax.c:1793-1797). Not tracked. TASK-91.4 sees the pgbench load blocked in fxfs_dax_iomap_write -> map_extents. Related: TASK-91.4, TASK-79.28, TASK-79.53.
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
2026-10-24: SQLite WAL/NORMAL batch1 supplies the clean reproducer: almost no device I/O, with fxfs_dax_map_write at 19.44% and transaction commit at 9.72% of guest workload samples (docs/measurements/gce-fs-bench-2026-10-24T2105Z-07519990). Tracked as the first step of TASK-92.58.
<!-- SECTION:NOTES:END -->
