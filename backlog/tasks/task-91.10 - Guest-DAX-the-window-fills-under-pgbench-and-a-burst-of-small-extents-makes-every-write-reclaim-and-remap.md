---
id: TASK-91.10
title: >-
  Guest DAX: the window fills under pgbench, and a burst of small extents makes
  every write reclaim and remap
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 22:01'
updated_date: '2026-10-24 01:53'
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
- [x] #2 fxfs-guest-pgbench-dax's third run within 10% of its first
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Evacuation (user approved 2026-10-23): at a seal, staging regions are marked evacuate and the files with writable pages are recorded; once the seal ends, a work item gathers those files' ranges' pages in marked regions, maps them read-only in the ranges' own space in one MAP_MANY, relocates them there (unless rewritten meanwhile or another iomap uses the range), and prunes the regions they emptied. No protocol or host change. Proof: fxfs-guest-checks-dax regions_evacuated (400 pages written once, fsynced each: regions bounded), red on vmlinux-drv3; pgbench-dax MAPs per run, regions_full, tps.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
a8e2e02c: regions have the window's last half to themselves, reclaiming nothing (full: -EAGAIN, the piece mapped in a range's own space). Fragmentation was the cause, not fullness: with one pool the largest free run stayed ~2 MiB with 180-590 MiB free; failed allocations ~300,000 in a run, ~2,000 after. A quarter held run 3 but slowed the load to 35 s; half keeps it at 23-24 s. GCE, full guest suite passes (defaults, both pgbench, crash, follow, daemon-lost, live-fork); DAX pgbench 1,035 / 862 / 807 tps (was 1,002 / 859 / 29-171). AC2 not met: run 3 is 78% of run 1, the fall being between runs 1 and 2 (seen with every window layout; without DAX the runs hold ~1,000), still to explain.

Round 5 (sandbox team, a8e2e02, 2026-10-23): no collapse in 20 runs; DAX tps still falls 27-31% over five runs at 25 ms and 0 ms alike (1,081 / 856 / 793 / 744 / 742). MAP requests flat at 79-87k a run, so MAPs per transaction rise from ~1.3 to ~1.9; MAP time 12 to 15 s; Postgres user time -8.6 s, guest system time +4 s; UNMAPs 1-4k a run (not the window collapse). Their ask: find why MAPs per transaction rise; done when five DAX runs at 25 ms hold within 10% of the first.

With /proc/fs/fxfs (263a3c34), DAX pgbench runs 1-3: commits 0.58 / 0.70 / 0.77 ms, MAPs per commit 1.96 / 2.49 / 2.64 (MAP 482-569 us as the guest sees it, 189-271 us in extentd), DAX stores held by seals 7.5 / 12.6 / 15.4 s a run, the window 'full' ~100k times a run, of which most are the regions' space (pieces then map in ranges' own space: ~80k such MAPs a run). The host side's growth (extent store commit 20 to 98 us, MAP 189 to 271 us) is the review session's extent_store work (review-r1: TASK-92.16 and others), to remeasure once on main.

590e1cef: ranges let go of regions their pages left. DAX pgbench (GCE r63): regions bounded ~1,450 over five runs (was growing to 5,400-20,000), 30,336 pruned; tps unchanged 922/777/736/725/634. New stats show why: the regions' half (1 GiB of a 2 GiB window) is full from the load step on, ~1,450 regions of ~0.6 MB of the load's data, largest free run 2 MiB; window_regions_full ~100k a run, writes fall back to range MAPs (~70k a run); the ranges' half full too (largest free 1.3 MB, ~150-400 reclaims a run). Remaining DAX cost is likely per-MAP: TASK-92.50 (host map_unsafe O(n) in the VMO's mappings) is being fixed by the review session; re-measure after it before more window work. Part 2 of option 1 (moving small sealed regions' pages into ranges) would add range MAPs, so held until then.

(The commit above is 009ae677.)

2026-10-23, sandbox round 7 at beedb64 (DAX 2 GiB 975->802 tps, 8 GiB 1021->801; ~92k MAPs a run) and our runs at vmlinux-drv3 (GCE r63). Their 'window full' counts the ranges' pool alone; the regions' pool is full 75-99k times a run here, and 54-72k of a run's 66-73k MAPs are range MAPs. With an 8 GiB window (experiment, not committed): the load seals ~200 times; run 1 seals 38,960 times and makes ~30,000 regions (869 -> 28,568; regions' pool 4 GiB -> 351 MB free), pinned by pages written once (accounts rows, history), which pruning can't free; from run 2 the regions' pool is full and writes MAP their pages into ranges (44-67k a run), tps 1018/802/750/759/725. A larger window delays the fall-back, no more: regions save a write's MAP only while the pool has room, and moving cold pages out of small sealed regions (option 1's second half) costs the same MAPs. Next: the per-MAP cost (TASK-92.50, TASK-91.16: ~40-60 us in extentd against ~400 us the guest waits); then, if needed, the device moving a seal's staging pages to their ranges' addresses as part of the seal's remap, so a written page takes no MAP of its own.

30407e2a: evacuation. DAX pgbench (GCE r63, vmlinux-evac1): regions 360-540, regions' pool full 0-1 a run, range MAPs 66-143 a run; tps 890/780/758/727/698, run 5 -22% (was -29%). Per run: commits 630-731 us (no DAX 363-410 us), MAP requests 614-793 us, 58-72k a run (restage ~28k + evacuation ~30k, off the write path, 44-46 s), stores held by seals 7-13 s. Left: one MAP request per seal (restage and evacuation in one MAP_MANY), and the DAX commit's extra ~300 us (the host's seal remap, the review session's seal-in-place reprotect on round 2b).
<!-- SECTION:NOTES:END -->
