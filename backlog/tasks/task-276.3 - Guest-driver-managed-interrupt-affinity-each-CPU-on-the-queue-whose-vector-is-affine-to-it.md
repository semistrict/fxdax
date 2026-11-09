---
id: TASK-276.3
title: >-
  Guest driver: managed interrupt affinity, each CPU on the queue whose vector
  is affine to it
status: To Do
assignee: []
created_date: '2026-11-09 03:53'
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
- [ ] #1 The driver passes struct irq_affinity (pre_vectors for config) and maps CPUs to queues by the vectors affinity, as blk_mq_virtio_map_queues does
- [ ] #2 The bench shows completions landing on the submitting CPU (cross-CPU share near 0 with a queue per CPU)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
