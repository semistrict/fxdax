---
id: TASK-92.18
title: Mount replays the journal one 4 KiB block per serial device round trip
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
ordinal: 293000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fill_buf awaits one block per loop iteration (reader.rs:190-197), and read_aligned awaits one device read per extent, scanning from the first extent (bootstrap_handle.rs:126-145). fxdax makes every journal block its own extent (journal.rs:98-101), the journal can reach 8 MiB (fs.rs:346-347), and renew_tail reads its tail extents serially (journal.rs:1505-1518). A fork mount makes about 700 serial READs, about 100 ms of the 0.34 s mount; a full journal would take up to 2,048.

Where: crates/fxfs/src/object_store/journal/reader.rs:190-197, crates/fxfs/src/object_store/journal/bootstrap_handle.rs:126-145, crates/fxfs/src/object_store/journal.rs:98-101, crates/fxfs/src/object_store/journal.rs:1505-1518, guest/rust/fxfs-linux/src/fs.rs:346-347.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there; the journal code changed only in 05d1dd5. fill_buf awaits one read_aligned per 4 KiB block (crates/fxfs/src/object_store/journal/reader.rs:190-197). BootstrapObjectHandle::read_aligned scans the extents from the first and awaits one device read per extent (bootstrap_handle.rs:126-152). EXTENT_SIZE is still 4096, so each journal block is its own extent (journal.rs:98-101). renew_tail reads the tail extents serially (journal.rs:1500-1523). JOURNAL_MAX is 8 MiB (guest/rust/fxfs-linux/src/fs.rs:62, 346-347). TASK-33 would avoid replay for prepared bases but does not speed replay up. Related: TASK-33.
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
The journal reader now reads ahead of the blocks it verifies, in windows that double up to 32 blocks, and the bootstrap handle reads all of a window's extents at once. Read-ahead through extents that are truncated, discarded or replaced is forgotten. renew_tail reads the first blocks of the tail's extents at once. Measured with FakeDevice's round-trip counter, a mount replaying 256 journal blocks went from 323 round trips to 16, and renew_tail over 61 extents from 61 to 1. The tests pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
