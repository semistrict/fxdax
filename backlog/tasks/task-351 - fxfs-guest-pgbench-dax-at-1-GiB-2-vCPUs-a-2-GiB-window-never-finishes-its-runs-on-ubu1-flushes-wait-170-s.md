---
id: TASK-351
title: >-
  fxfs-guest-pgbench-dax at 1 GiB, 2 vCPUs, a 2 GiB window never finishes its
  runs on ubu1: flushes wait 170 s
status: To Do
assignee: []
created_date: '2026-11-11 11:12'
labels:
  - bug
  - testing
dependencies: []
priority: medium
ordinal: 741000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-344's proving run of scripts/soak-host.sh (2026-11-11, commit 397e9eff2, one cell 1024:2:2048, one round, FXDAX_REMOTE_LATENCY_MS=25, ubu1 under other agents' load, the stores on /mnt/fxdax-scratch): the case loaded its database (OK load 5000000) and then made none of its 5 runs before its 1200 s limit (round-1.json: expected OK runs 5, took 864 s of the runner's view; the guest's sysrq dump shows postgres and kworkers in D). The run's extentd stats: durable_lag_gt_16384ms=24, durable_lag_max_us=33361057, flush_max_wait_us=170729449, flush_max_data_us=27406286, framing_max_write_us=27953751, written_raw_bytes 8.9 GB, written_copied 1.6 GB. Find whether the host's disk under load alone explains a 170 s flush wait or the store stalls (a flush waiting on a framing, the uploader, the bucket's latency), and make the case pass or say why it can't on a loaded host. Reproduce: FXDAX_SOAK_ROUNDS=1 scripts/soak-host.sh RESULTS 1024:2:2048.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause of the 170 s flush wait is named, from the run's extentd log and stats
- [ ] #2 fxfs-guest-pgbench-dax at 1024:2:2048 passes on ubu1, or the case's limits say why a loaded host can't
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
