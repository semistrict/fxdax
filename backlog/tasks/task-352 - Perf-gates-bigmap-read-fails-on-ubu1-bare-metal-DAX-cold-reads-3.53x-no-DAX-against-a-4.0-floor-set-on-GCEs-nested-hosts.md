---
id: TASK-352
title: >-
  Perf gate's bigmap-read fails on ubu1, bare metal: DAX cold reads 3.53x no-DAX
  against a 4.0 floor set on GCE's nested hosts
status: To Do
assignee: []
created_date: '2026-11-11 11:44'
labels:
  - performance
dependencies: []
priority: medium
ordinal: 742000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-344's proving run of scripts/perf-gate-host.sh on ubu1 (2026-11-11, commit 397e9eff2, bare metal, fxjob --bench, FXDAX_PGBENCH_RUNS=1, FXDAX_PERF_GATE_VMS=2, FXDAX_PERF_GATE_DENSITY_RUNS=1, other agents loading the host's other cores): every case of perf-single and perf-density passed and every gate but one: bigmap-read, bigmap-dax's cold read 211.8 MiB/s against bigmap-no-dax's 60.0 MiB/s, 3.53x, under scripts/pgbench-gate.py's floor of 4.0. The gates' thresholds were set from GCE runs on nested hosts (r632, docs/measurements/dax-cow-gate.md); ubu1 is bare metal, where pgbench-gate.py reports numbers apart (--host bare-metal) but applies the same thresholds. Measure it again on ubu1 alone (fxjob --bench, no other measurement), and either find why DAX's cold read is slower against the page cache's on bare metal or set bare metal's threshold from bare-metal runs, recorded in dax-cow-gate.md.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 bigmap-read on ubu1 measured in at least three bench runs, the numbers recorded
- [ ] #2 The gate passes on ubu1, by a fix or a bare-metal threshold justified by those runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
