---
id: TASK-44.18
title: >-
  Production-shaped guest soaks: small guest memory, more vCPUs than cores,
  small windows, bucket latency, rounds
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-31 08:40'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 397000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team's 2 GiB DAX VM hit TASK-99 under memory pressure the suites don't create: they run 4 GiB guests, 2 vCPUs, 2 GiB windows. FXDAX_PGBENCH_MEMORY_MIB, _VCPUS and _WINDOW_MIB set them for pgbench; a nightly matrix of such shapes, several rounds each, finds what a fresh small VM meets.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A GCE driver runs the DAX suites over a matrix of guest memory, vCPUs and window sizes, with bucket latency, several rounds each
- [x] #2 Its results keep each round's measurements and every failure's logs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31, scripts/soak-gce.sh, default matrix, 25 ms latency, 3 rounds a cell (tps of the five runs): 1024:2:2048 1042 949 916 866 886; 1024:4:2048 1062 970 920 794 816; 2048:4:8192 1062 989 976 923 966; 4096:2:2048 1032 948 912 847 882, each round alike. 2048:4:512 passed 3 of 3 once pg_ctl waited for crash recovery past its 60 s: its window, smaller than the database, falls from 252 tps to 68 over five runs: a window that small runs a sixth as fast and gets worse. No cell's refused stores unexplained. scripts/soak-workloads-gce.sh runs the rest of the suites.

Correction: the rerun's 2048:4:512 rounds: 316 139 108 67 58, 320 133 107 67 57, 320 135 108 67 59; the failed first run's 252 to 68.
<!-- SECTION:NOTES:END -->
