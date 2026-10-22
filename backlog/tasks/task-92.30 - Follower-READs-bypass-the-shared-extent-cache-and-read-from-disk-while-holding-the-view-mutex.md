---
id: TASK-92.30
title: >-
  Follower READs bypass the shared extent cache and read from disk while holding
  the view mutex
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
ordinal: 305000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
read_cached returns false for followers (extents.rs:998-1000), so each READ holds following.view across read_view, an O_DIRECT pread or a 1 MiB unpack (1029-1034; store.rs:2601-2610). Yet follower MAPs already use the cache after a view check (1436-1453). N followers of a pgbench writer each pay 50-150 µs of disk per READ instead of a 1-2 µs copy, and their READs and ADVANCE serialize behind it.

Where: crates/fxdax/src/extents.rs:998-1000, crates/fxdax/src/extents.rs:1029-1034, crates/fxdax/src/extents.rs:1436-1453, crates/extent_store/src/store.rs:1605-1618, crates/extent_store/src/store.rs:2601-2610.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. read_cached returns false for followers (crates/fxdax/src/extents.rs:939-942). read_stored then holds following.view.lock() across store.read_view, the disk read or unpack (extents.rs:970-976). The follower MAP path already checks view_extent under the view and then uses the cache (extents.rs:1386-1390). Not tracked. Related: TASK-32, TASK-42.3, TASK-79.7.
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
