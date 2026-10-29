---
id: TASK-131.5
title: >-
  CI performance benchmark: one entry point, five-run DAX/no-DAX pgbench and a
  density test, machine-readable, failing on regressions, against ext4 dax on
  pmem
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 476000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: own the performance benchmark in CI: one entry point, a five-run DAX/no-DAX pgbench and a density test, with machine-readable output; pass/fail on run-5 tps against run 1, DAX against no DAX, and one long-lived extentd serving many VMs; measured against bench-fs-ext4-dax-pmem, so the gap to a static DAX device is measured. Bare-metal and nested numbers reported apart (window remaps and first-touch mapping cost much more nested). Builds on TASK-111.2 (one pgbench entry point) and TASK-112.2 (ext4 dax on pmem, bare metal and nested).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One command runs it on GCE, bare metal and nested apart, writes JSON, and fails on each threshold
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
