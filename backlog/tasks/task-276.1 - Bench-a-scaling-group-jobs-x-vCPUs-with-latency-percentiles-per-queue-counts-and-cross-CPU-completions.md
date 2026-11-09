---
id: TASK-276.1
title: >-
  Bench: a scaling group (jobs x vCPUs) with latency percentiles, per-queue
  counts and cross-CPU completions
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 03:53'
updated_date: '2026-11-09 09:18'
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
- [x] #1 FXDAX_BENCH_VCPUS sets the bench VMs vCPUs (default unchanged, 2)
- [x] #2 A scaling group runs rand-read cold and warm, rand-write, fsync-append and concurrent create/stat/unlink at FXDAX_BENCH_JOBS jobs (default 1,2,4,8), each reporting throughput and fio clat p50/p99
- [x] #3 Each scaling workload observes the extent device per-queue requests and the share of queue interrupts that land on a CPU other than the queues own
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FXDAX_BENCH_VCPUS and the scaling group (FXDAX_BENCH_JOBS) in the fs bench, with fio p50/p99, per-queue requests, interrupts by CPU and the driver's cross-CPU completion count; and the interleaved A/B tooling (scripts/bench-ab-gce.sh, bench-ab.py --control, test-e2e.sh's prebuilt artifacts). Used for every TASK-276 measurement on GCE; scripts tests pass; main CI 42590395981 green.
<!-- SECTION:FINAL_SUMMARY:END -->
