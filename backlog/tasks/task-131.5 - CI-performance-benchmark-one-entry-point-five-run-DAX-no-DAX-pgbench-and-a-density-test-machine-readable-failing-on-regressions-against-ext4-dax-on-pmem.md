---
id: TASK-131.5
title: >-
  CI performance benchmark: one entry point, five-run DAX/no-DAX pgbench and a
  density test, machine-readable, failing on regressions, against ext4 dax on
  pmem
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-30 03:30'
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

GCE r261 at 9601cd2ba, scripts/pgbench-gate.py: no-DAX tps 1636.6, 1672.2, 1685.9, 1727.2, 1691.8 (decay 1.0337, passed); DAX 1041.1, 945.6, 878.9, 778.4, 825.9 (decay 0.7933, failed; DAX/no-DAX 0.5213, failed). DAX is half as fast as no DAX on GCE nested.

On main (dee940204, ad8aeef22): scripts/perf-gate-gce.sh [--bare-metal] INSTANCE RESULTS runs fxfs-guest-pgbench, fxfs-guest-pgbench-dax and pgbench-ext4-dax-pmem (new: the pgbench case on ext4 mounted dax=always on virtio-pmem, a copy of a base image, asserted mounted with DAX) one VM each into RESULTS/single, then a density run of FXDAX_PERF_GATE_VMS (default 3) DAX VMs on one daemon into RESULTS/density, then pgbench-gate.py over both into RESULTS/gate.json; non-zero on a failed run or gate. The gate now reads several results, reports static-dax (DAX fxfs median against ext4 DAX on pmem; FXDAX_GATE_STATIC_DAX, default 0, ungated), gates decay and density on the fxfs cases only, takes density from the first round of more than one VM, and counts each VM's runs from stdout.txt (the guest's OK runs lines were never in log.txt, so density had never split VMs). Tests: scripts/tests/test_pgbench_gate.py, test_perf_gate_gce.py (fake test-gce.sh); scripts suite 117 OK. GCE r261 (nested n2-standard-8, release kernel, ce3b003db, FXDAX_PERF_GATE_VMS=2): no-DAX tps 1673.1, 1706.7, 1706.7, 1699.1, 1681.8 (decay 1.0052); DAX 1050.8, 948.6, 872.5, 796.2, 827.7 (decay 0.7877 FAIL; DAX/no-DAX 0.5135 FAIL); ext4 DAX on pmem 1180.1, 1104.7, 1187.0, 1190.9, 1197.4 (DAX fxfs/ext4-dax-pmem 0.7351); density 2 VMs: VM 1 median 888.8, VM 2 803.9 (0.9045, pass). Open: bare metal needs a metal machine and quota.

GCE r264, scripts/perf-gate-gce.sh at cda03e8f8 (main, buddy allocator): no-DAX 1422-1469 tps (decay 0.998), DAX 1020, 898, 881, 813, 866 (decay 0.8495, failed; DAX/no-DAX 0.6135, failed; r261 before the allocator 0.51), ext4-dax-pmem 1125-1171 (DAX against it 0.7688), density 0.9787 passed.
<!-- SECTION:NOTES:END -->
