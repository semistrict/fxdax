---
id: TASK-116
title: >-
  Guest DAX: a failed evacuee_add mid-gather leaves the file's region
  unevacuated until its next seal (suspected leak)
status: To Do
assignee: []
created_date: '2026-10-28 12:45'
updated_date: '2026-10-30 18:05'
labels:
  - bug
dependencies: []
ordinal: 433000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28): fxfs_dax_evacuee_add failing mid-gather returns silently with earlier runs held and later regions unevacuated. A file never written again keeps the region; under memory pressure region space leaks. Unverified.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Confirmed or refuted; if confirmed, the region is retried or released without a further seal of that file
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-30, read of dax.c, no test: not a leak of space. evacuee_add failing returns with the runs gathered so far, which the evacuation places as usual; the later regions stay their range's, region->evacuate still set, until the file's next seal, but fxfs_dax_reclaim takes any range no iomap uses (fxfs_dax_reclaim_range, fxfs_dax_unmap_range), the regions with it, once the window's regions fill (window_regions_full). So under window pressure the space comes back without a seal of that file. krealloc_array failing mid-gather also leaves ev consistent: runs grown, maps not, capacity unchanged. No test fails that allocation; AC1 left unchecked until fxfs-guest-faults-dax or a knob exercises it.
<!-- SECTION:NOTES:END -->
