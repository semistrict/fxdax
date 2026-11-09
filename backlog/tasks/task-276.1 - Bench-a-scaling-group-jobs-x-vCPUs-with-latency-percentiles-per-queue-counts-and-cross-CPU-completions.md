---
id: TASK-276.1
title: >-
  Bench: a scaling group (jobs x vCPUs) with latency percentiles, per-queue
  counts and cross-CPU completions
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 03:53'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: task
ordinal: 703000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fs bench runs every case on 2 vCPUs, one fio job and 4 metadata threads, and reports throughput alone: it cannot show how fxdax scales with jobs or vCPUs, nor where completions land. TASK-276 needs that baseline.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FXDAX_BENCH_VCPUS sets the bench VMs vCPUs (default unchanged, 2)
- [ ] #2 A scaling group runs rand-read cold and warm, rand-write, fsync-append and concurrent create/stat/unlink at FXDAX_BENCH_JOBS jobs (default 1,2,4,8), each reporting throughput and fio clat p50/p99
- [ ] #3 Each scaling workload observes the extent device per-queue requests and the share of queue interrupts that land on a CPU other than the queues own
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
