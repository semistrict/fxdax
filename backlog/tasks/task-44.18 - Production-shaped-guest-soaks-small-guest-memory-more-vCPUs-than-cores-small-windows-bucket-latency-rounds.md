---
id: TASK-44.18
title: >-
  Production-shaped guest soaks: small guest memory, more vCPUs than cores,
  small windows, bucket latency, rounds
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
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
- [ ] #1 A GCE driver runs the DAX suites over a matrix of guest memory, vCPUs and window sizes, with bucket latency, several rounds each
- [ ] #2 Its results keep each round's measurements and every failure's logs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
