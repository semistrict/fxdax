---
id: TASK-276.8
title: >-
  Affine queue interrupts: one job's 16-deep io_uring cold reads on fxfs-guest,
  p99 +20%
status: To Do
assignee: []
created_date: '2026-11-09 07:08'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
priority: low
type: bug
ordinal: 712000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-276 affinity A/B (one build, virtio_extent.affine=0 vs 1, 4 vCPUs, 3 interleaved rounds x 2 iterations, GCE c3-standard-8): scale-rand-read-cold-qd16 on fxfs-guest p99 1,163 us (1,036-1,237) -> 1,401 us (1,319-1,565), 1.20x against the ext4 control's 1.09x: real by the A/B's rule; its IOPS 29.7k -> 27.7k, within drift. Every other workload improved or held (cold-read p50 -11%, DAX random writes +14-17%, fsync p50 -8-12%). One job submitting 16 reads through io_uring has them served by io-wq workers on several CPUs; with affine vectors each completes on its submitter's CPU, without, on the next CPU. Why the deep queue's tail grows is not known.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause of the qd16 p99 rise under affine vectors is measured (per-CPU completions and io-wq placement)
- [ ] #2 Fixed, or recorded as the cost of affinity with the evidence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
