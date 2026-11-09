---
id: TASK-276.9
title: >-
  8 vCPUs, 8 jobs: fsync p99 of fsynced appends up 1.20x with TASK-276's queues
  and affinity
status: To Do
assignee: []
created_date: '2026-11-09 10:12'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
priority: low
type: bug
ordinal: 714000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 landed-tree A/B at 8 vCPUs (main before it + bench vs the landed tree, 3 interleaved rounds x 2 iterations, ext4 control, GCE c3-standard-8, docs/measurements/virtio-pci-parallelism-2026-11-09/ab-landed-v8): fxfs-guest-dax scale-fsync-append-j8-fsync-p99 13.2 ms (12.4-14.5) -> 15.9 ms (15.3-17.7), 1.20x against the control's 1.00x: real. fxfs-guest the same way, 1.28x, within drift (ranges overlap). The same workloads' fsync p50 fell 16-25% and their IOPS rose 9-20% in the same A/B, and at 4 vCPUs the p99 held: a tail on the oversubscribed host (8 guest vCPUs on 8 host vCPUs), cause unknown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The p99 rise is explained (per-CPU completions, commit waits at the tail) and fixed, or recorded as the cost of the change with the evidence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
