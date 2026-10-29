---
id: TASK-131.5
title: >-
  CI performance benchmark: one entry point, five-run DAX/no-DAX pgbench and a
  density test, machine-readable, failing on regressions, against ext4 dax on
  pmem
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-29 21:57'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29: scripts/pgbench-gate.py (on main): reads a test-gce.sh results directory's fxfs-guest-pgbench and -dax logs (rounds too, and FXDAX_PGBENCH_VMS's VMs), writes JSON (host kind, tps by case, round, VM), and fails on decay (last run against first, 0.9), DAX against no DAX (medians, 0.95) and density (one daemon's last VM against its first, 0.9), FXDAX_GATE_* overriding. Against r264 (b5d3246) it fails decay at 0.73. Open: the one GCE command running the cases and the gate; ext4 dax on pmem beside it (bench-gce.sh's bench-fs-ext4-dax-pmem, a separate harness); bare metal: GCE's bare-metal machines (c3-*-metal) are 192 vCPUs, past the project's 32-CPU quota, so it needs a quota raise or another host.
<!-- SECTION:NOTES:END -->
