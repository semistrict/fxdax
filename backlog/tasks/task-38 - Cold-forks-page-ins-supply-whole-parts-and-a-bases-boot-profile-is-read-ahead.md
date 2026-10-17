---
id: TASK-38
title: >-
  Cold forks: page-ins supply whole parts, and a base's boot profile is read
  ahead
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 16:25'
updated_date: '2026-10-17 16:36'
labels:
  - performance
  - forks
dependencies: []
priority: high
ordinal: 105000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-30 measured a VM booted on a fork of a base image (docs/measurements/gce-fs-bench-fork-2026-10-17T1609Z): with the base's data in the extent daemon's memory the fork reads the base's /usr in 0.57 s, without it 6.16 s; PostgreSQL starts in 0.15 against 0.25 s; pgbench -S runs at 46.7k against 20.6k tps. Cold, the daemon paged in 1.27 GB in 9.7k page-ins of 128 KiB (FxExtent's read-ahead), each decompressing the 1 MiB part it falls in (extent_frame), and the guest took 37.9k DAX faults. Two levers: a page-in that decompresses a part could supply all of the part's pages it is allowed to, so that a cold read faults and decompresses once per part rather than up to 8 times; and what forks of a base read at boot could be recorded, and read into the daemon's cache before the next fork reads it (doc-7, Reading: profiles), so that a cold fork starts as a warm one does. Scope: the disk image (extentd, the extent cache's pager, extent_store); the bench cases bench-fork-fxfs-guest-dax-{warm,cold} measure both.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A page-in of a sealed extent stored compressed decompresses each part once, and supplies the part's pages, within the extent and the pager's limits: a cold fork's page-ins and decompressions fall, shown by extentd's counters in the fork bench
- [ ] #2 Forks of a base record which of its extents, and ranges, they read in their first seconds, and a new fork's daemon reads them into its cache before or as the fork boots
- [ ] #3 A cold fork with the profile read ahead reads the base's /usr, starts PostgreSQL and runs pgbench -S within 1.5x of a warm fork on GCE, measured with a third fork case, and without it no slower than now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count decompressions of parts (extent_store), reported with extentd's counters.
2. Page-ins of sealed extents read 1 MiB (extent_frame::PART) at once rather than 128 KiB: the source says how much (ExtentSource), so that a page-in decompresses a part once and supplies it.
3. Measure the cold and warm fork cases on GCE against the TASK-30 run.
4. Then the boot profile read ahead, with a third fork case.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step 2 (1d41a40), GCE (docs/measurements/gce-fs-bench-fork-parts-2026-10-17T1631Z, against gce-fs-bench-fork-2026-10-17T1609Z): cold fork page-ins 9.7k -> 1.37k, read /usr 6.16 -> 5.71 s; DAX faults 37.9k -> 36.7k, fault time ~13 s summed: faults, not page-ins, are what a cold fork waits for now. Each fault maps the faulting page and at most 16 more already present (the kernel's optimistic pages), so a part paged in whole still takes ~15 faults to map. Unpacks 2.0k for 1.37k page-ins: an extent's 1 MiB is not part-aligned in its segment, so a page-in spans two parts, some unpacked twice. Warm unchanged (0 page-ins, 117 faults). pgbench -S noisy in both runs.
<!-- SECTION:NOTES:END -->
