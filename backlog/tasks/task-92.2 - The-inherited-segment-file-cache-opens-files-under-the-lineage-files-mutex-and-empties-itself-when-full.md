---
id: TASK-92.2
title: >-
  The inherited-segment file cache opens files under the lineage files mutex,
  and empties itself when full
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
ordinal: 277000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On a miss, the file is opened (dir.rs:113-130) and Framed::open runs: an fstat and two O_DIRECT preads, including a footer read, CRC-checked and discarded (framed.rs:62-76, 97). This happens under the mutex every inherited read takes, hits included (lineage.rs:355-360), while the caller holds its state lock (store.rs:1526->1591->1273). Once 64 files are open, retain(strong_count > 1) drops nearly all of them (361-363), and each reopen orphans its Unpacked keys (framed.rs:91). Cold page-ins across a base over about 2 GiB miss about 75% of the time, at 0.2-3 ms each, serialized for every VM of the base.

Where: crates/extent_store/src/lineage.rs:355-363, crates/extent_store/src/framed.rs:62-76, crates/extent_store/src/framed.rs:91, crates/extent_store/src/framed.rs:97, crates/extent_store/src/dir.rs:113-130, crates/extent_store/src/store.rs:1273, crates/extent_store/src/store.rs:1526, crates/extent_store/src/store.rs:1591.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. Lineage::segment_file opens the file and runs Framed::open under the files mutex. Every inherited read takes that mutex, hits included (crates/extent_store/src/lineage.rs:198-210). When full it still does files.retain(strong_count > 1) (206). Framed::open -> read_frame still does an fstat, a trailer read and a tail read with check_footer (framed.rs:62-77, 95-100). Each opening gets a new OPENED id, which orphans its Unpacked keys (framed.rs:80-92). The branch's own caches keep the same policy: OwnFiles::file retain (state.rs:381-395) and Segments::close_idle_files (segments.rs:356-371). TASK-79.38 is the correctness side of this cache (never invalidated on removal). Related: TASK-79.38, TASK-34.
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
A file of a segment the lineage's forks inherited now opens outside the lineage's mutex, so one fork's opening no longer stalls the other forks' reads. The file caches (the lineage's, OwnFiles, Segments') now share an LRU (framed::Files) that closes the least recently read idle file instead of emptying itself when full. Opening a framed file now reads its trailer and table, one block, instead of the footer as well. In the measured workload, files opened fell from 17 or 20 to 8 and bytes read from 208,896 or 245,760 to 65,536, and a Shuttle test that deadlocked before now passes. All of this passes on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
