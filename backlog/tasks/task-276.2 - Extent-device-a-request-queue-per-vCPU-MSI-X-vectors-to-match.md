---
id: TASK-276.2
title: 'Extent device: a request queue per vCPU, MSI-X vectors to match'
status: To Do
assignee: []
created_date: '2026-11-09 03:53'
labels:
  - firecracker
  - perf
dependencies: []
parent_task_id: TASK-276
type: enhancement
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The device offers EXTENT_NUM_QUEUES = 4 queues whatever the VM has, so CPUs past the fourth share queues (and their locks, and their VMM workers) with others. With PCI each queue has its own MSI-X vector.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The device offers as many request queues as the VM has vCPUs, bounded by a maximum; MSI-X vectors are queues + 1
- [ ] #2 A snapshot taken by the same build restores with its queue count
- [ ] #3 Unit tests cover the count and its bound
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
