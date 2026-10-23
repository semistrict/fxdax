---
id: TASK-92.1
title: >-
  Seal reads back and rewrites every extent that took more than one WRITE, while
  holding the store's locks
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
priority: high
type: enhancement
ordinal: 276000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
seal leaves an extent in place only if it is one piece (store.rs:2043-2049), and finish_write never merges pieces (1458-1466). So a multi-WRITE extent is read back whole through the O_DIRECT fd, after a forced writeback (dir.rs:83), and rewritten (2056-2078) inside commit_locked, holding committing, state and open. The guest's 1008 KiB max_io (device.rs:554-571) splits every 1 MiB Fxfs LSM flush (data_object_handle.rs:2628) in two. A 20 MiB flush holds the store for tens of ms up to about 100 ms, and those bytes are stored and uploaded twice.

Where: crates/extent_store/src/store.rs:2043-2049, crates/extent_store/src/store.rs:2056-2078, crates/extent_store/src/store.rs:1458-1466, crates/extent_store/src/dir.rs:83, guest/rust/fxfs-linux/src/device.rs:554-571, crates/fxfs/src/object_store/data_object_handle.rs:2628.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. finish_write still appends each WRITE as a separate Piece with no merge (store.rs:646-676, pieces.push at 669). seal still keeps an extent in place only if it is a single run (store.rs:1106-1113). Otherwise it reads the whole extent back via parts_locked/read_parts and rewrites it into the open segment or its own file (store.rs:1116-1143). This happens inside commit_in (store.rs:946-950), under committing and state. No task tracks it. TASK-91.5 (1.57 GB stored for 0.8 GB) lists write-amplification causes but not this double write. Related: TASK-91.5, TASK-91.4, TASK-61.
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
A write that continues its extent's last piece, next to it in both the extent and the segment and in either order, now joins that piece; recovery's replay joins it the same way. So the guest's 1 MiB flushes, split at 1008 KiB, are sealed where they are rather than read back and rewritten under the commit's locks. extents_written_in_runs_seal_where_they_are shows the commit's I/O falling from 2 MiB read and 2 MiB written to none. It passes on macOS and on Linux (GCE). Extents whose writes are not one run are still copied at the seal.
<!-- SECTION:FINAL_SUMMARY:END -->
