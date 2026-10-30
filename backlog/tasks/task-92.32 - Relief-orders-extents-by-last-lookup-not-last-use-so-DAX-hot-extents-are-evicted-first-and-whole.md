---
id: TASK-92.32
title: >-
  Relief orders extents by last lookup, not last use, so DAX-hot extents are
  evicted first, and whole
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
ordinal: 307000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
used is stamped only in cached() (extents.rs:1708), never on window faults or page-ins (dax.rs:296-332), and guests keep their DAX mappings, so hot DAX data keeps the stamp of its first MAP. evict_least_used sorts by that stamp (138) and evicts victims whole, up to 64 MiB (579) for a 4 MiB request (150-151, 122), and kernel page ages cannot correct it (vm_aspace.rs:172; mmu.rs:377). Under a cap, a base's boot extents that every fork maps are evicted first, unmapped from every window, refault, and are evicted again: the working set thrashes.

Where: crates/fxdax/src/extents.rs:122, crates/fxdax/src/extents.rs:138, crates/fxdax/src/extents.rs:141, crates/fxdax/src/extents.rs:150-151, crates/fxdax/src/extents.rs:579, crates/fxdax/src/extents.rs:1708, crates/fxdax/src/dax.rs:296-332, crates/zircon/src/vm/vm_aspace.rs:172, crates/zircon/src/arch/host/mmu.rs:377.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present. Relief moved into the PMM (crates/zircon/src/vm/pmm/relief.rs, cbe6c42), but the evictor is the same evict_least_used (crates/fxdax/src/extents.rs:126-154, registered via relieve_with at 159-170). Cached.used is stamped only in cached() (extents.rs:1657), never on faults or page-ins. Victims are sorted by that stamp (135) and evicted whole (op_range 0..size, 146-148) to meet RELIEF_BYTES = 2*CHUNK_SIZE (relief.rs:29). No preference for unmapped extents. Not tracked. Related: TASK-50.3, TASK-26.15, TASK-87.
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
Relief orders extents by whether a window maps them, then by when last used, a fault counting (cache.rs, CachedExtent::eviction_order). Tests: relief_evicts_what_no_window_maps_first, relief_evicts_what_was_faulted_on_longest_ago_first, each failing without it (mutant: (0, 64) for (64, 0) held). macOS and Linux, GCE r273. Whole extents still go for a smaller request.
<!-- SECTION:NOTES:END -->
