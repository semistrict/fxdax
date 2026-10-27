---
id: TASK-109.2
title: >-
  extentd: with an 8 GiB DAX window, MAP goes 79 us -> 4.4 ms and seal 70 us ->
  2.7 ms in pgbench's third run, CPU-bound
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-25 23:26'
updated_date: '2026-10-27 14:44'
labels: []
dependencies: []
parent_task_id: TASK-109
priority: high
ordinal: 411000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team, b746104, 8 GiB DAX VM on one 8 GiB extentd: run 2 969 tps, 148,416 MAPs at 79 us each, seal phase 2.6 s, extentd io user 18 s; run 3 336 tps, 49,629 MAPs at 4.4 ms (216 s total), seal phase 32 s, extentd io user 41 s, guest idle 84 of 120 vCPU-s, with 6.5 GB of window free and no window_full. Unprofiled: the profiled retry died of the EFAULT (TASK-101). Candidates: TASK-92.50 (map_unsafe SPECIFIC_OVERWRITE growing with the VMO's mappings) and TASK-108.1 (extentd at its memory cap). Their repro: SMITHBOX_TEST_PGBENCH_PROFILE=1 SMITHBOX_TEST_PGBENCH_HOSTPROF=1 SMITHBOX_TEST_PGBENCH_PROFILE_RUNS=all SMITHBOX_TEST_PGBENCH_RUNTIMES=fxdax-dax-8g bash smith-go/sandboxes/ci/fxdax-pgbench.sh.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A profile of extentd during the slow run attributes the per-MAP growth
- [ ] #2 fxfs-guest-pgbench-dax with an 8 GiB window on GCE shows extentd's MAP and seal time flat across five runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-27, sandbox team's profiled repro at bf8eedc (8 GiB window, frame pointers, perf): from run 3 on, 58-65% of sampled CPU is fxdax::extents::let_go_least_used's collect, called from cached() on every map, commit and read once the cache passes MOST_CACHED (65,536): the window's mappings hold their extents (strong_count > 1), so the pass, a sort of every entry under the cache's lock, frees nothing, and runs again at the next lookup. A 2 GiB window reclaims, letting go of its mappings' extents; 4 and 8 GiB windows never fill, so never reclaim. Collapse independent of extentd's budget (24 GiB) and vm.max_map_count (1M). AC1 met by their profile.
Fix: TRIM_AT, the cache trims again only once it grows by an eighth of its bound past what its last pass left (when the pass gets below the bound, as before, at the bound); stats cache-trims and cache-trim-ns. Test the_cache_trims_what_windows_hold_once_per_eighth_of_its_bound: 256 trims for 256 lookups without it, at most 33 with. Left: AC2, an 8 GiB fxfs-guest-pgbench-dax on GCE, five runs.

r156 (GCE, the fix, 8 GiB window, FXDAX_PGBENCH_WINDOW_MIB=8192): tps 1126, 1046, 931, 1008, 998 over the five runs (theirs at bf8eedc: 1005, 915, 339, 248). Guest window MAP per run ~295, ~300, ~307, ~310, ~312 us; seal ~487 us in run 1, ~514 us in run 5. extentd: 705,939 MAPs at 66 us mean over load and runs; cache-trims 4 in all (8 ms). window_reclaims 0, as theirs. AC2 met.
<!-- SECTION:NOTES:END -->
