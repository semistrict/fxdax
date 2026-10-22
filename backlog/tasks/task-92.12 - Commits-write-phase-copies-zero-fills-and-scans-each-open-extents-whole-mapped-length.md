---
id: TASK-92.12
title: >-
  Commit's write phase copies, zero-fills and scans each open extent's whole
  mapped length
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
ordinal: 287000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
commit_and_seal allocates vec![0; len], copies the whole VMO (extents.rs:1226-1227), scans back for the last non-zero block (486-492), and Store::write zero-fills and copies again (store.rs:2508-2514). len is the end of the writable mapping (1522-1524), committed eagerly (1532) up to 64 pages ahead of the data (dax.c:78-84), and the work runs under the open mutex. In pgbench this phase is about 30 µs per fsync for about 30 KiB; an estimated 20 µs is avoidable, about 20% of extentd's WRITE.

Where: crates/fxdax/src/extents.rs:1226-1234, crates/fxdax/src/extents.rs:486-492, crates/fxdax/src/extents.rs:1522-1524, crates/fxdax/src/extents.rs:1532, crates/extent_store/src/store.rs:2508-2514, crates/extent_store/src/framed.rs:29-33, guest/linux/fs/fxfs/dax.c:78-84.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. commit_and_seal does vec![0; len], vmo.read of the whole mapped length, then kept_len scans back for the last non-zero block (crates/fxdax/src/extents.rs:1164-1177, kept_len at ~471), all under the open mutex. Store::write zero-allocates an Aligned buffer and copies again (crates/extent_store/src/store.rs:1434-1441). There is no high-water mark from WRITE tracking, and extents never written are not skipped. Related: TASK-41, TASK-91.4, TASK-26.11.
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
