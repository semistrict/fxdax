---
id: TASK-92.33
title: >-
  Each 4 MiB relief pass clones, sorts and calls info() on every cached extent
  of every arena, under the process-wide cache mutex
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-30 10:18'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 308000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Each pass collects every entry of every arena under cache.extents (extents.rs:131-137) and sorts them (138). It then calls vmo.info(), two page-list walks, from the oldest up (146-149), revisiting evicted entries that stay in the map until a FREE or evict_arena (1111-1119, 231-239). One relief thread serves every arena (169-187) while allocations wait (pmm.rs:550-559), and MAP and READ take the same mutex (1689). At 10^4-10^5 entries, each pass holds the global mutex for milliseconds, and reclaim is capped at tens to hundreds of MiB/s.

Where: crates/fxdax/src/extents.rs:131-149, crates/fxdax/src/extents.rs:169-187, crates/fxdax/src/extents.rs:231-239, crates/fxdax/src/extents.rs:1111-1119, crates/fxdax/src/extents.rs:1689, crates/zircon/src/object/vm_object_dispatcher.rs:303-321, crates/zircon/src/vm/vm_cow_pages.rs:4049-4071, crates/zircon/src/vm/pmm.rs:550-559.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present, unchanged. Every entry of every arena is cloned under cache.extents (crates/fxdax/src/extents.rs:129-134), then sorted (135), and vmo.info() is called on each from the oldest (143). Entries emptied by eviction stay until a FREE or evict_arena (200-208). One PMM relief thread serves every arena (crates/zircon/src/vm/pmm/relief.rs:89-110), and MAP/READ/replay take the same cache.extents mutex in cached() (extents.rs:1636-1658). Not tracked. Related: TASK-50.3, TASK-79.7, TASK-87.
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
Relief leaves out extents with nothing in memory, by a count of present pages, and asks each its order past the cache's lock; every entry is still cloned under that lock. relief_evicts_what_no_window_maps_first checks eviction_order is None once evicted. GCE r273.
<!-- SECTION:NOTES:END -->
