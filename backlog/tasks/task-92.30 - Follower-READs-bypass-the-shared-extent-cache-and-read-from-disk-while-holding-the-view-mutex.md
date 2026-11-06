---
id: TASK-92.30
title: >-
  Follower READs bypass the shared extent cache and read from disk while holding
  the view mutex
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-06 00:48'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 305000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
At dde8d58d3: read_cached returns false for followers (crates/fxdax/src/extents.rs:939-942), so each READ holds following.view across the store's read_view, a disk read or an unpack (read_stored, extents.rs:970-978; Store::read_view, crates/extent_store/src/store.rs:2191-2200). Yet follower MAPs already use the cache after a view check (extents.rs:1335-1340). N followers of a pgbench writer each pay 50-150 µs of disk per READ instead of a 1-2 µs copy, and their READs and ADVANCE serialize behind it.

Where: crates/fxdax/src/extents.rs:939-942, 970-978, 1335-1340; crates/extent_store/src/store.rs:2191-2200.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. Related: TASK-32, TASK-42.3, TASK-79.7.
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
Frozen 2026-11-04 by the user: followers and the standalone store layer have no customer now; resumed after read-only DAX (TASK-164) and the storage collapse (TASK-165) land (docs/reviews/2026-11-04-first-principles.md, section 6).

2026-11-05 triage: citations brought to dde8d58d3; still frozen.
<!-- SECTION:NOTES:END -->
