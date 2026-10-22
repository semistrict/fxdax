---
id: TASK-92.23
title: Every sync drops the whole file's page cache with FADV_DONTNEED
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 298000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
HostFile::sync follows each fdatasync with posix_fadvise(DONTNEED) over the whole file (dir.rs:276-284), and every durable flush syncs the open segment (store.rs:1047, 2346-2349). A page still in another CPU's LRU-add batch makes the kernel run lru_add_drain_all, cross-CPU work done under a global mutex. If most flushes hit it, pgbench's ~1,360 durable commits/s each pay tens to hundreds of µs. Unmeasured.

Where: crates/extent_store/src/dir.rs:275-284, crates/extent_store/src/store.rs:1047, crates/extent_store/src/store.rs:2346-2349.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. HostFile::sync runs fdatasync and then posix_fadvise(0,0,DONTNEED) over the whole file whenever it is written through the page cache (crates/extent_store/src/dir.rs:288-299). Each durable flush syncs the open segment if it was written since the last commit: Segments::take_syncs (segments.rs:376-384) feeds make_durable, then Syncs::sync (store.rs:1308-1327; durability.rs:110-121). Introduced by c065b46 (TASK-27.4). Not tracked. Related: TASK-27.4, TASK-26.18.4.
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
