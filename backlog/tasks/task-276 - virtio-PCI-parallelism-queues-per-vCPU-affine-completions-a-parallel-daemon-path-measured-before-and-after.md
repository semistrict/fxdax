---
id: TASK-276
title: >-
  virtio-PCI parallelism: queues per vCPU, affine completions, a parallel daemon
  path, measured before and after
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 03:53'
labels:
  - firecracker
  - perf
dependencies: []
priority: high
type: enhancement
ordinal: 702000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every fxdax VM runs on virtio-PCI since TASK-273, whose MSI-X gives each queue its own vector, but the extent device still offers a fixed 4 request queues (EXTENT_NUM_QUEUES), the guest driver asks for no interrupt affinity (a completion may interrupt a CPU other than its submitter), and every queue worker of a device shares one connection to extentd. docs/measurements/virtio-pci-vs-mmio-2026-11-09.md measured single-job workloads only: how fxdax scales with jobs and vCPUs, and where it stops, is unmeasured. This measures the scaling on one GCE host (fio random reads cold and warm, random writes, fsync-heavy, concurrent metadata; 1-8 jobs; 2, 4 and 8 vCPUs; fxfs-guest, fxfs-guest-dax, ext4 on virtio-blk as control), removes the ceilings it finds, and measures again on the same host.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Baseline scaling of current main measured on one GCE host and its ceilings identified (throughput, p50/p99, host-side profile, per-queue counts, cross-CPU completions)
- [ ] #2 Each optimization subtask done or skipped with the measured reason
- [ ] #3 Same host, same matrix, after: docs/measurements/virtio-pci-parallelism-<date>.md has baseline, per-optimization deltas, final scaling and regressions
- [ ] #4 Firecracker changes on a new branch and series tag on semistrict/firecracker; series check passes both ways; test_config_space_as_upstream passes
- [ ] #5 Release and debug guest rounds pass on GCE; main CI green
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
