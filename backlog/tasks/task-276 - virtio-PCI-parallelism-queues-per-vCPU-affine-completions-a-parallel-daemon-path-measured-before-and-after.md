---
id: TASK-276
title: >-
  virtio-PCI parallelism: queues per vCPU, affine completions, a parallel daemon
  path, measured before and after
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 03:53'
updated_date: '2026-11-09 05:52'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Phase 1 (baseline, main 0f011561a + bench 3be9d1bf8, GCE fxdax-keep-c3-standard-8-us-east5-b-1, nested KVM; results in the session scratchpad base-v2/v4/v8/base-prof, to go into docs/measurements):
- Completions: 100% of fxfs requests complete on another CPU than their submitter at every vCPU count and job count: queue i's MSI-X vector is affine to CPU i+1 (default vector placement, the config vector first), the driver sends CPU i to queue i.
- DAX random 4K writes: flat ~10-12k IOPS from 2 jobs on, any vCPU count, latency linear in jobs (p50 140 us j1 -> 725 us j8). Each write is a PRIVATE; the VMM's one extentd-client thread applies every stage operation in order and is saturated: 47% mmap over installed pages and 13% fallocate, both dominated by TLB-shootdown IPIs (smp_call_function_many_cond 41%), 7.6% KVM_PRE_FAULT_MEMORY, 7% recvmsg. mmap holds mmap_lock for write, so more threads would not overlap it.
- Non-DAX cold 4K reads: per-READ 150-190 us at every job count (extentd's own read 157 us): latency-bound, not serialized; scales 3.6k -> 19-20k IOPS (j1 -> j8) at 2-4 vCPUs. The client thread spends a third of its time in COW faults of fresh guest memory (wp_page_copy -> TLB shootdown) as it copies read data in.
- Daemon connection: the client pipelines (4096 in flight), extentd serves 8 a connection at once; neither saturates.
- ext4 on single-queue virtio-blk: cold reads 27-30k (j1) -> 48-89k (j8), above fxfs everywhere: no single-queue ceiling that matters to fxdax (TASK-276.5 skip).
- fsync-append: ~4-5k IOPS for any jobs/vCPUs, fsync p99 12-17 ms at j8: commit serialization, outside the queues.
- 8 vCPUs on an 8-vCPU host: everything drops (host oversubscribed).

Coordination (2026-11-09): TASK-277's agent tags fxdax-v1.16.1-r3 (branch fxdax-v1.16.1-pci-vsock-reset, vsock reset before snapshot). Don't use r3. On landing: rebase this task's Firecracker commit (bb2d858a1 on fxdax-v1.16.1-pci-mq, queue per vCPU) onto whatever main's vendor/firecracker points at then, series check both ways, tag fxdax-v1.16.1-r4. The A/B results stand: the vsock change doesn't touch the extent device's I/O path.
<!-- SECTION:NOTES:END -->
