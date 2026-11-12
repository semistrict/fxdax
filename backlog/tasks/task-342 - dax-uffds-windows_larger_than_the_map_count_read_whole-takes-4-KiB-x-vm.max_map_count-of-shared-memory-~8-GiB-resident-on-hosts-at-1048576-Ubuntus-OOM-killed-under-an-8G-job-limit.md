---
id: TASK-342
title: >-
  dax::uffd's windows_larger_than_the_map_count_read_whole takes 4 KiB x
  vm.max_map_count of shared memory: ~8 GiB resident on hosts at 1,048,576
  (Ubuntu's), OOM-killed under an 8G job limit
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 02:08'
updated_date: '2026-11-12 08:06'
labels:
  - tests
dependencies: []
type: bug
ordinal: 737000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-276.6 on ubu1 (vm.max_map_count 1048576): the test's window is max_map_count + MAPPINGS_RESERVED pages, each a run of its own, and it reads them all (twice), configuring the arenas at 4x that rounded to a GiB (16 GiB). Under fxjob's default 8G memory limit it was killed (Memory cgroup out of memory: shmem-rss 8.26 GB), passing in 52 s under 20G. A host's vm.max_map_count decides the test's memory: bound it (a budget below the host's, as with_budget gives others) so that it reads past the budget without taking the host's map count in memory.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The test passes under an 8 GiB memory limit on a host with vm.max_map_count 1048576, still reading more scattered pages than its budget of runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Budget the test by Linux's default vm.max_map_count (65,530), or the host's if lower, through with_budget; window of that count + MAPPINGS_RESERVED scattered pages (> budget, asserted).
2. Run it alone on ubu1 under fxjob --mem 8G; record peak memory and time.
3. macOS cargo nextest of fxdax.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
ubu1 (vm.max_map_count 1048576), alone under fxjob --mem 8G: passed in 5.45 s (job 7.2 s), scope memory peak 600.8 MB, where at the host's count it took 8.26 GB shmem and was OOM-killed at 8G (52 s under 20G). The window is 65,530 + 4,096 = 69,626 scattered pages, a run each, against a budget of 30,717 runs (asserted pages > budget), read whole twice. macOS cargo nextest run -p fxdax: 337 passed, 6 skipped (the test is Linux's).

DoD #1: macOS fxdax suite passed; on Linux only this test ran (as asked), not the workspace.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
windows_larger_than_the_map_count_read_whole budgets by Linux's default vm.max_map_count (65,530), or the host's if lower, through with_budget, not the host's own: 69,626 scattered pages against 30,717 runs, read twice, the assertion unchanged. On ubu1 under fxjob --mem 8G it passes in 5.5 s at a 601 MB peak (was OOM-killed at 8.26 GB).
<!-- SECTION:FINAL_SUMMARY:END -->
