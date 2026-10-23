---
id: TASK-91.10
title: >-
  Guest DAX: the window fills under pgbench, and a burst of small extents makes
  every write reclaim and remap
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 22:01'
updated_date: '2026-10-23 00:03'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 316000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured with a guest kernel logging the window every 5 s (pgbench -s 50 on fxfs-guest-pgbench-dax, 2 GiB window, GCE, 2026-10-22; the patch is in the scratchpad as dax-window-debug.patch). The window is full from 16 s into the load: 71 MiB of 2,048 free, then ~150 MiB free through the first two 60 s runs while take_window fails ~2,000 times a second and ranges are reclaimed. In the third run the live regions (one per extent the window maps, kept while any range slices it) jump from ~3,000 to ~22,000 within 20 s, likely a Postgres checkpoint writing its dirty buffers, fsynced file by file, each seal moving pages to new small extents; from then on most time goes to reclaim and remap: ~2,400 MAPs/s, 15-20 commits/s, 128 tps (1,002 and 859 before). Without DAX the same runs hold ~1,000 tps.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Window use under pgbench -s 50 with a 2 GiB window stays below its size, or reclaiming keeps up without remap storms, shown by a window-usage measurement
- [ ] #2 fxfs-guest-pgbench-dax's third run within 10% of its first
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
a8e2e02c: regions have the window's last half to themselves, reclaiming nothing (full: -EAGAIN, the piece mapped in a range's own space). Fragmentation was the cause, not fullness: with one pool the largest free run stayed ~2 MiB with 180-590 MiB free; failed allocations ~300,000 in a run, ~2,000 after. A quarter held run 3 but slowed the load to 35 s; half keeps it at 23-24 s. GCE, full guest suite passes (defaults, both pgbench, crash, follow, daemon-lost, live-fork); DAX pgbench 1,035 / 862 / 807 tps (was 1,002 / 859 / 29-171). AC2 not met: run 3 is 78% of run 1, the fall being between runs 1 and 2 (seen with every window layout; without DAX the runs hold ~1,000), still to explain.
<!-- SECTION:NOTES:END -->
