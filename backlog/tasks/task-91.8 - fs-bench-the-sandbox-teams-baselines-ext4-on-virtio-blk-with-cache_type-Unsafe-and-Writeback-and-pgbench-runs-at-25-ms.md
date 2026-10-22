---
id: TASK-91.8
title: >-
  fs-bench: the sandbox team's baselines, ext4 on virtio-blk with cache_type
  Unsafe and Writeback, and pgbench runs at 25 ms
status: To Do
assignee: []
created_date: '2026-10-22 18:22'
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
