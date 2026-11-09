---
id: TASK-276.3
title: >-
  Guest driver: managed interrupt affinity, each CPU on the queue whose vector
  is affine to it
status: Done
assignee: []
created_date: '2026-11-09 03:53'
updated_date: '2026-11-09 09:18'
labels:
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
virtio_extent asks virtio_find_vqs for no irq_affinity and picks queues[cpu % nr_queues]: a queues completion interrupt may land on any CPU, not the submitters, costing an IPI to wake it and a cache miss on its request.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The driver passes struct irq_affinity (pre_vectors for config) and maps CPUs to queues by the vectors affinity, as blk_mq_virtio_map_queues does
- [x] #2 The bench shows completions landing on the submitting CPU (cross-CPU share near 0 with a queue per CPU)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
virtio_extent asks for managed interrupt affinity (the config vector before the queues') and maps each CPU to the queue whose vector is affine to it; module parameters affine and max_queues turn it back for A/Bs. Cross-CPU completions 100% -> 0%. A/B (one build, 4 vCPUs, 6 a side, ext4 control): cold-read p50 -11 to -12%, DAX random writes +14 to +17% (1-4 jobs), fsync p50 -8 to -12%; one-job 16-deep io_uring read p99 +20% (TASK-276.8). Release and debug guest rounds, the 11 PCI cases, main CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
