---
id: TASK-276.2
title: 'Extent device: a request queue per vCPU, MSI-X vectors to match'
status: Done
assignee: []
created_date: '2026-11-09 03:53'
updated_date: '2026-11-09 09:18'
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
- [x] #1 The device offers as many request queues as the VM has vCPUs, bounded by a maximum; MSI-X vectors are queues + 1
- [x] #2 A snapshot taken by the same build restores with its queue count
- [x] #3 Unit tests cover the count and its bound
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The extent device offers a request queue per vCPU (request_queues: 1..=32), MSI-X vectors queues + 1; a restore takes the snapshot's count. Firecracker fxdax-v1.16.1-r4 (a075570c8, branch fxdax-v1.16.1-pci-mq-r4, on TASK-277's r3). Verified: a_queue_per_vcpu and the 23 extent tests, series check both ways, vmm suite with KVM 774/775 (TASK-271's test_set_tsc), fxfs-guest-snapshot-dax 10/10 on the landed tree, main CI green incl. firecracker-series. Alone (A/B, 8 vCPUs, 4 vs 8 queues): fsync p50 -6 to -15%, throughput within drift.
<!-- SECTION:FINAL_SUMMARY:END -->
