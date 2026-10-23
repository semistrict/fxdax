---
id: TASK-92.36
title: Image import writes file contents one 4 KiB block at a time
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
ordinal: 311000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
write_contents calls write_all once per non-zero 4 KiB block, even for contiguous blocks (import.rs:263-266). Each write makes its own DIRTY pager request (vm_cow_pages.rs:1793-1800), where one coalesced write could request 256 pages (vm_object_paged.rs:762-765), and each file allocates a zeroed 1 MiB buffer (258). That is about 262k round trips per GiB, about 3-5 s/GiB of base building.

Where: crates/fxdax/src/import.rs:52, crates/fxdax/src/import.rs:258, crates/fxdax/src/import.rs:263-266, crates/fxdax/src/filesystem.rs:754-772, crates/zircon/src/vm/vm_object_paged.rs:762-765, crates/zircon/src/vm/vm_cow_pages.rs:1793-1800.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. write_contents allocates vec![0; CHUNK] (1 MiB) per file (crates/fxdax/src/import.rs:385). It calls write_all once per non-zero HOLE=4096 block, even for contiguous runs (import.rs:390-394). Each call is a separate Node::write stream write (filesystem.rs:671-684). Not tracked. Related: TASK-39.5, TASK-79.56.
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
Image import now writes each run of a file's data blocks with one write instead of one write per 4 KiB block. Fxfs's pager is asked for a run's pages together, every file is read through one buffer the importer keeps, and each file's size comes from where its last write ended instead of a getattr. import-bench on macOS went from 804-1164 to 3373-4761 MiB/s for a 256 MiB file, and instructions retired over the bench fell from 56.1-57.4 G to 50.6-51.1 G. Behaviour tests for runs, files longer than a chunk and all-zero files pass on macOS and Linux (GCE). During this work the implementer saw a 1 GiB import stall under the default 1 GiB kernel memory, outside the importer; that is a separate issue.
<!-- SECTION:FINAL_SUMMARY:END -->
