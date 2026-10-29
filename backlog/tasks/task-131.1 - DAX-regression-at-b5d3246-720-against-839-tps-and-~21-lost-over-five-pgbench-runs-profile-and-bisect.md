---
id: TASK-131.1
title: >-
  DAX regression at b5d3246: 720 against 839 tps, and ~21% lost over five
  pgbench runs: profile and bisect
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-29 22:07'
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
<!-- SECTION:NOTES:END -->
