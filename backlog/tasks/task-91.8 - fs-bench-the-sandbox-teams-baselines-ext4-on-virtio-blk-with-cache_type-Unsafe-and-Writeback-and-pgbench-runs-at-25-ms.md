---
id: TASK-91.8
title: >-
  fs-bench: the sandbox team's baselines, ext4 on virtio-blk with cache_type
  Unsafe and Writeback, and pgbench runs at 25 ms
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-22 18:22'
updated_date: '2026-11-08 06:38'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 273000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team's runtime today is ext4 on virtio-blk with flushes dropped (Firecracker cache_type Unsafe); the durable comparison is cache_type Writeback (each guest flush a host fsync). They ask for both beside ext4-dax-pmem in fxdax's bench, pgbench as two 60 s runs at 25 ms object-store latency, reported per step as their harness does (load phases, tps per run, guest CPU, extentd requests, commit wait).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fs-bench has both virtio-blk cases and runs pgbench at 25 ms as two 60 s runs
- [ ] #2 The report shows per-step numbers: load phases, tps per run, guest user/system/idle, extentd requests and commit wait
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: on land-w12 (70f0283c8, CI 42455538348). pgbench-ext4-blk-unsafe and pgbench-ext4-blk-writeback in the perf gate's single phase (tag perf-single) beside pgbench-ext4-dax-pmem; runs as FXDAX_PGBENCH_RUNS/RUN_SECS say (2 x 60 s for the sandbox team's), the Fxfs cases' bucket 25 ms away; pgbench-gate.py reports unsafe-dax and writeback-dax ungated; scripts/pgbench-steps.py's per-step report (load phases, tps and latency per run, guest user/system/idle/steal, extentd requests by kind and commits with their wait), written by the gate's last phase to steps.md. GCE r1022 (c3-standard-8), 2 x 60 s, jailed: Unsafe 3,609/3,820 tps (load 10.87 s, idle 9%), Writeback 1,536/1,534 (15.16 s, idle 55-57%), ext4-dax-pmem 1,263/1,163 (17.48 s), Fxfs 2,011/2,037 (13.46 s), Fxfs DAX 2,004/2,047 (13.45 s, ~3,340 WRITEs/s at 31-38 us, ~1,370 commits/s waiting 5-20 us). docs/measurements/gce-pgbench-baselines-2026-11-08 (steps.md).
<!-- SECTION:NOTES:END -->
