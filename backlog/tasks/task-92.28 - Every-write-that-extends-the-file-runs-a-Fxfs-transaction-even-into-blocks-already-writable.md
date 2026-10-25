---
id: TASK-92.28
title: >-
  Every write that extends the file runs a Fxfs transaction, even into blocks
  already writable
status: In Progress
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-25 00:34'
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
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24: SQLite WAL/NORMAL batch1 supplies the clean reproducer: almost no device I/O, with fxfs_dax_map_write at 19.44% and transaction commit at 9.72% of guest workload samples (docs/measurements/gce-fs-bench-2026-10-24T2105Z-07519990). Tracked as the first step of TASK-92.58.

2026-10-25: Implemented writable-page append reuse, metadata-only deferred size persistence, serialization of writeback with truncate, and size persistence before COW/reflink/resize. A new grow-after-append test exposed a window/device coherence defect: partial-page ordinary grow now seals and drops its replaced mapping first. All five guest cases, four append checks, and macOS/Linux workspace suites passed. Controlled same-host A/B in both orders (10 observations per kernel/workload) gives batch100 gains of 10.1% DELETE/FULL, 10.4% WAL/FULL, and 13.3% WAL/NORMAL; WAL/NORMAL batch1 is effectively flat (+1.4% with broad overlapping ranges). The append test confirms zero subsequent map-write transactions, but the targeted small-transaction benefit remains unproven, so performance acceptance and task status remain open. Evidence: docs/measurements/sqlite-dax-appends-2026-10-24.md.
<!-- SECTION:NOTES:END -->
