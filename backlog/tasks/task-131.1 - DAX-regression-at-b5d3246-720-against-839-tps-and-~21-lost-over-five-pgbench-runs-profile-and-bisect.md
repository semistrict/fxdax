---
id: TASK-131.1
title: >-
  DAX regression at b5d3246: 720 against 839 tps, and ~21% lost over five
  pgbench runs: profile and bisect
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-30 13:43'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 472000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: DAX got 14% slower at b5d3246 (720 tps against 839), and loses ~21% over five runs; their guess is the seal correctness work. Profile a five-run pgbench-dax at b5d3246 and at the commit before the slowdown; bisect between; separate the first run's tps from the decline over runs (TASK-111.1's fault growth).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The commit or commits costing the tps are named, with profiles before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, GCE n2 8-CPU, release guest kernel, pgbench-dax 5 runs (tps): c6d9b7e (r263) 1042 939 916 860 888; b5d3246 (r264) 922 811 795 645 670: 11-27% lower, decaying faster. Host counters over the whole test: the VMM's window changes (stage-ops) 1.94M at c6d9b7e against 5.36M at b5d3246, 10 per commit against 36; stage maps 1.04M against 5.78M for 43.9 GB against 52.7 GB, 42 KB a run against 9 KB: the window's runs of physical pages are a fifth as long; stage overwrites 244K against 1.07M, write-protects 198K against 898K; MAPs 3.7 per commit against 6.1. Guest: window map and unmap 314/319 us against 363/395 us each, run 5 291/311 against 387/496 us; seals 489 against 561 us, run 5 528 against 733 us. Suspects: f897ce4a2 (fxdax's own physical memory, replacing the Zircon port's: runs shorter) and ff7961411 (MAP_AHEAD). Bisecting: 6a08c8622 (f897ce4a2's parent) on r263, 79a4ab93e (f897ce4a2 with guest builds fixed) on r264.

Also: the VMM's 'extentd client' thread, which makes the window's changes (mmap, mprotect), used 169 s of CPU (21 user, 148 sys) over the test at b5d3246 against 89 s (12, 77) at c6d9b7e; the vCPU threads 284+246 s user at b5d3246. 6a08c8622 and 79a4ab93e rerun with vendor/firecracker initialized (the first attempts shipped it empty: stale or failed builds).

79a4ab93e (r264, vendor/firecracker initialized): 913 816 801 649 680 tps; stage-ops 5.42M, stage maps 5.81M for 52.6 GB, VMM 'extentd client' 21+150 s: the whole regression is there. Pending: 6a08c8622 (f897ce4a2's parent, r263), ff7961411 (MAP_AHEAD, r261), f34c0b75c (midpoint, r264).

Bisected: 6a08c8622 (f897ce4a2's parent, r263): 1037 941 895 870 898 tps, stage-ops 2.70M, stage maps 1.58M for 55.7 GB (35 KB a run), VMM 'extentd client' 16+102 s; 79a4ab93e (f897ce4a2 with guest builds fixed): 913 816 801 649 680, 5.42M, 5.81M for 52.6 GB (9 KB a run), 21+150 s. The regression is f897ce4a2, the Zircon port's deletion: fxdax's own physmem, extent cache and DAX windows. Its windows map in runs of physically consecutive pages (Window::install, map_run), each run a stage map; a seal's read-only remap of an open extent's pages is reprotected in place in the VMM (uffd remap), but one reprotect per physical run (write-protects 198K against 898K). With physical runs a quarter as long, every kind of window change roughly quadrupled. Open extents' pages are allocated a page at a time (OpenPages::zeroed), interleaving with every other extent allocating at once. A/B on GCE: a27e50fd8 (r264) against it with open extents' pages allocated in runs after the page before them (branch wip-open-pages-runs, r263). Next if that's not all: coalesce the VMM's in-place reprotects of window-consecutive runs, and have the window protect pages whose frames don't change rather than remap them.

A/B: open extents' pages allocated in runs (wip-open-pages-runs, r263) left runs as short, 5.75M stage maps for 54.7 GB (9.5 KB), against 5.69M for 51.9 GB at a27e50fd8 (r264): not the cause; dropped. tps 949..696 against 905..668, within hosts' spread. Committed meanwhile: the VMM reprotects in-place maps one after another in the window at once (verified by tests; perf to measure).

GCE r264 at 641568faf, pgbench-dax 1 VM: tps 828.5, 715.6, 768.9, 664.7, 666.7. stage-maps 5030410 (47.5 GB, 9.4 KB each) in stage-segments 1181078 (40 KB each): a window-consecutive run of pages present together is 4.3 runs of physical memory, so the short maps come from scattered physical memory, not from gaps in the window. stage-maps-open 1605892 (32%); the rest are cached extents read in with alloc_run. stage-ops 4752917 at 25.6 us each (121.9 s); guest MAP requests 825265 at 93 us each (76.9 s). Next: physmem's free list is LIFO and alloc_run only extends a run by the physically next free page, so churn scatters free memory; reproduce in a physmem test, then an allocator that keeps free runs whole (buddy).

GCE r264 at 585a82004 (641568faf + open extents' pages allocated in runs after the page before them, wip-open-pages-runs): tps 927.7, 815.0, 817.0, 672.9, 673.5; stage-maps 5609050 in stage-segments 1327152 (4.23 per segment; 4.26 without), 9.5 KB each. Allocating in runs doesn't lengthen them. r264-F counts open extents' allocations, their pages and the physical runs they came as, to tell page-at-a-time commits from scattered free memory.

GCE r264 at 4782583c0 (open pages in runs + allocation counters): tps 937.9, 829.0, 801.9, 661.6, 653.4. Open extents' allocations: 292262, 4554810 pages (15.6 each), in 1739511 physical runs (2.6 pages each). Commits ask for runs; free memory is fragmented: the arenas' free list, LIFO, hands out the short runs freed last (an fsync's small extents) first. Fix: physmem's free pages as a buddy allocator's blocks (0b846e695, local): runs from the smallest block holding them, single pages from the smallest blocks, freed pages joined with their buddies. physmem tests (54, shuttle 10) and fxdax's pass on macOS. r264-G measures it with open pages in runs.

GCE r264 at ed4f7745e (buddy allocator 0b846e695 + open pages in runs + counters), pgbench-dax: tps 1018.9, 933.9, 891.1, 837.5, 893.2 (mean 915; r264 without: 938, 829, 802, 662, 653, mean 769: +19%; run 5/run 1 0.88 against 0.70). Open allocations 323024 of 5116569 pages in 325434 physical runs (1.007 a allocation; 6.0 without). stage-maps 2106558 (27.7 KB each; 9.5 KB without) in 1461628 segments (1.44 a segment; 4.23); stage-ops 2849107 (5243814), the VMM's stage time 83.9 s (130.4 s). Release suites and Linux workspace tests running before the push (r264, r263).

Pushed: e88f2c02a (buddy allocator), 9706c857f (open pages in runs, a chunk's at most). Verified: GCE r264 suites at ccbc757e1, fxfs-guest, -dax, -checks, -checks-dax, -crash, -crash-dax, -checks-dax-small 2 rounds each passed; Linux workspace r263 1959 of 1960 (the one TASK-132's flake, fixed since); macOS physmem and fxdax 292 passed on main with it.

GCE r261 at b6051f15c (main 3358b1089 + open extents taking 16 pages ahead of their next run, physically after their last), pgbench-dax: tps 1169.7, 1076.0, 1003.3, 988.0, 1036.4 against 1170.3, 1024.6, 1005.6, 985.0, 1017.3 at 8fc401ea9 (r261, batch 2): within the spread. Stage maps a window MAP over the run's fifths 2.84 1.72 2.06 1.99 1.89 against 2.83 1.72 2.01 1.95 1.80; stage segments a MAP 1.41..1.33 against 1.41..1.35; the VMM's stage time a change 19.9 27.1 21.5 24.0 16.7 us against 19.8 27.9 22.1 24.4 17.3. Open extents' physical runs an allocation rose (1.01..1.21 against 1.01..1.09): the pages ahead, cleared whenever the next commit isn't at the page after, split runs rather than join them. Open extents' maps are 0.28-0.34 a MAP, ~15% of stage maps; the rest are cached extents'. Not pushed: the reservation doesn't shorten what the windows map. Its unit test passes with it and fails without (AHEAD_PAGES = 0), so it does what it says; the workload's open extents don't grow in the way it serves.

Per pgbench run (batch 2 and batch 5 alike, r261): what decays is how much a commit changes the window, not what a change costs. Run 1 to 5: MAPs a commit 3.00 3.56 3.89 3.92 3.75, UNMAPs 1.04 1.03 1.43 1.53 1.41, stage changes 9.3 10.0 12.7 12.7 11.6, frees 0.21..0.36; a MAP 43-49 us, an UNMAP 17-21 us, the VMM's stage time 10-11 s a run: flat. Guest: window_maps_reach 4742, 5028, 20150, 22386, 18608 for 613 MB, 988 MB, 954 MB, 1045 MB, 836 MB, 130-196 KB each in runs 1-2 and 40-50 KB after; window_regions_read falls from run 3 (-1644 in run 4) while the window keeps 820 MB free: not capacity. No-DAX (1.00) and ext4 DAX on pmem don't decay (r264), so it is ours. Hypothesis: Postgres's first checkpoint rewrites heap pages into small extents, so reads map many small extents where they mapped the load's large ones. Test: r272, pgbench-dax with checkpoints off (experiment, not for main).

r272, pgbench-dax with checkpoints off (checkpoint_timeout=1d, max_wal_size=100GB, experiment 2cf31e1f9): tps 1075.4, 977.1, 985.9, 990.8, 1000.9; window_maps_reach 3294 3432 3077 2928 2940 (with checkpoints 4742 5028 20150 22386 18608), window_unmaps 41-45K a run (50-61K), window_regions_read +322 +86 +66 +91 +119 (+357 +265 +102 -1644 -769). So checkpoints cause the churn: their rewrites of heap pages are what breaks up the read regions. But tps barely gains: run 5 1001 against 1017 with them, run 1 lower, 1075 against 1170, on different hosts. What decays is not simply that churn: the runs flat from run 2 at the level the checkpointed runs fall to. Compare run 1, which alone stands above it. Its pg_ctl restart after the runs failed: recovering all the WAL took past pg_ctl's 60 s, the experiment's own doing.

perf kvm guest profiles at main, 15 s from 40 s (run 1, r287: tps 1162 1093 1028 1021 1043) and from 330 s (run 5, r288: 1184 1082 1012 992 1001), so a host each: the guest kernel's symbols add to 5.8% and 6.0% of samples, and none moves by more than 0.22 points: vm_interrupt 1.14 to 1.33, fxfs_dax_evacuee_gather 0.45 to 0.23, x2apic_send_IPI 0.69 to 0.72, vm_notify 0.64 to 0.67, __pv_queued_spin_lock_slowpath 0.20 to 0.21. The decay isn't in the guest kernel's code. Left: the rest of the samples, which the report doesn't name: Postgres in the guest, and the host's VMM and extentd threads; a host-side profile by thread, run 1 against run 5, would say.
<!-- SECTION:NOTES:END -->
