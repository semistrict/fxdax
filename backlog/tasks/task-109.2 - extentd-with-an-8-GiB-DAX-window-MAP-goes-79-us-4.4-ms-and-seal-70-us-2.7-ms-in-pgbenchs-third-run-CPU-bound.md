---
id: TASK-109.2
title: >-
  extentd: with an 8 GiB DAX window, MAP goes 79 us -> 4.4 ms and seal 70 us ->
  2.7 ms in pgbench's third run, CPU-bound
status: To Do
assignee: []
created_date: '2026-10-25 23:26'
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
