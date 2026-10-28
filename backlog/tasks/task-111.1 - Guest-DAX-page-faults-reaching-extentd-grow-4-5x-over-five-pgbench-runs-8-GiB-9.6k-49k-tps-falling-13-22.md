---
id: TASK-111.1
title: >-
  Guest DAX: page faults reaching extentd grow 4-5x over five pgbench runs (8
  GiB: 9.6k -> 49k), tps falling 13-22%
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 00:28'
updated_date: '2026-10-28 02:33'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: high
ordinal: 415000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the sandbox team round 7 (TASK-111), with DAX the guest page faults that reach extentd grow from 7,986 to 32,532 a run over five 60 s pgbench runs with a 2 GiB window and from 9,557 to 49,090 with 8 GiB, while tps falls 22% and 13%. At 8 GiB the window is never full and reach MAPs stay ~4k, so window pressure (the explanation in TASK-109 for 2 GiB) does not account for it; our own 8 GiB runs also fell (1168/1063/960/1045/1016 at a84611804). extentd time per fault stays ~10 us; its memory climbs toward its 8 GiB budget (3.8 -> 7.8 GB). No DAX shows none of it. Cause not shown: candidates are pages the window lets go of, or extentd evicts, being faulted back in.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause of the growth is identified and recorded
- [x] #2 Over five 60 s runs at 8 GiB, faults reaching extentd per run grow under 1.5x from run 1 to run 5
- [ ] #3 Run 5 tps is at least 90% of run 1 at 2 GiB and at 8 GiB
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Cause (from r201-8g at a84611804): extentd's faults are 0 until its memory reaches its 8 GiB budget mid run 2, then grow ~350/s; [EVICT] passes free 0.2-2 GiB at a time. The page scanner never harvests guest physical address spaces (as upstream), so a DAX window's pages are never aged as accessed after they are mapped: the guest's hot pages look oldest and are evicted first, then faulted back.
2. Fix: harvesting ages every page a guest physical aspace maps as accessed (UpdateAge, no harvest, in chunks of 512 pages per page-table lock hold); the aspace list holds every aspace, as upstream. Eviction takes pages no window maps first.
3. Test: guest_physical_mappings_are_aged_as_accessed_without_being_harvested (fails without).
4. Verify on GCE: fxfs-guest-pgbench-dax, 8 GiB and 2 GiB windows, five runs: extentd faults per run, tps run 5 vs run 1.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r201-111a (8 GiB, aging fix alone): faults still 39k, tps 1119/1004/920/970/979. r201-111b (new counters fault-stores, fault-refusals, fault-waits): of ~6k faults per 20 s after the 1.9 GiB eviction at t~190, 0 stores, 0 pager waits, 1/3 refused (KVM's async page-fault worker). So they are pages still in memory that the window lost the mapping of. Cause: Zircon's reclaim unmaps the whole aligned batch of 16 pages it looks at (UnmapAndHarvest) and keeps those too young to reclaim, unmapped; the window maps a page again only through a fault. Fix: reclaim_chosen_for_eviction picks the reclaimable pages first and unmaps only them; test evicting_a_page_leaves_the_mappings_of_those_it_keeps (fails without). r202-111a (2 GiB, aging fix): faults 12 (282 before), tps unchanged 1141/1041/981/941/946: the 2 GiB decline is window pressure, not faults. Sandbox rerun at c6d9b7e (prefault change): VMM 88-93 us per window change (was 114), DAX tps +1.6-6.5%, faults still grow ~4.5x.

r201-111c (8 GiB, both fixes): faults 6 in all (40-60k before), 0 pager waits; tps 1155/1067/1061/1033/1026 (run 5 89% of run 1; before 1125->969, 86%). r202-111c (2 GiB): faults 21, tps 1124/1015/974/920/953 (85%): window pressure. What remains of the 8 GiB decline follows extentd memory fragmenting as it recycles at its cap: KB per stage map 42 -> 23, stage ops per MAP 2.6 -> 3.3 (TASK-108.1's open item). AC3 not met. Validation: zircon and fxdax tests on macOS (445); Linux workspace on GCE r201 (2661 passed); DAX guest suites fxfs-guest-dax, fxfs-guest-checks-dax, extent-guest on r202 pass; upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->
