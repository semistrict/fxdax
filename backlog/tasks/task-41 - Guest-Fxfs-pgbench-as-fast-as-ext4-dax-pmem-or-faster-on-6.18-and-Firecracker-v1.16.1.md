---
id: TASK-41
title: >-
  Guest Fxfs: pgbench as fast as ext4-dax-pmem or faster, on 6.18 and
  Firecracker v1.16.1
status: To Do
assignee: []
created_date: '2026-10-17 21:20'
updated_date: '2026-10-20 22:06'
labels: []
dependencies: []
priority: medium
ordinal: 119000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On the 6.18 guest kernel and Firecracker v1.16.1 (TASK-39.6), pgbench on Fxfs (guest kernel, DAX, store in GCS) trails ext4 on DAX pmem: 1,466 tps vs 1,751 (1.19x), init 3.96 s vs 3.19 s (1.24x), docs/measurements/gce-fs-bench-pgbench-gcs-618. On 6.12 and libkrun Fxfs was ahead (1,376 vs 1,054 tps, docs/measurements/gce-fs-bench-pgbench-gcs): Fxfs barely moved, ext4 got 66% faster, possibly Firecracker's virtio-pmem or 6.18's DAX path. TASK-27.4 had pgbench within 1.5x; the goal now is parity or better.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pgbench-tps on fxfs-guest-dax is at least ext4-dax-pmem's, on GCE (c3-standard-8, 3 iterations), with the store in GCS as in gce-fs-bench-pgbench-gcs-618
- [ ] #2 pgbench-init on fxfs-guest-dax takes no longer than on ext4-dax-pmem in the same run
- [ ] #3 The run's results and what made the difference are kept in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 measurements (docs/measurements/gce-fs-bench-perf-*-2026-10-20*): baseline at 9d58058 pgbench 1,425 vs 1,708 tps (1.20x), init 1.30x. Guest profile through pgbench (perf-prof): guest 29% idle; per transaction 0.56 WRITEs (339 us, the fdatasync's journal commit), 1.08 FREEs (203 us), 0.56 MAPs (203 us: the staging extent restaged after each seal, in the background). Natively a write+ordered commit is ~90 us (gce-mapbench), and Firecracker's served times (Info log) put the host's side of a WRITE at ~300 us: the time is in the host, not the transport. Whole host profiled (perf-host): 78% idle, so it is latency, through ~6 thread hand-offs (vCPU, queue worker, server, extentd client, extentd request/io, back). Batch 1 (49dbcd3: EVENT_IDX, FREE_MANY, fdatasync without times): pgbench 1,441 (1.17x), unlink 2.73x -> 2.17x. Batch 2 (the VMM spinning 50 us for replies) gave nothing, reverted; guest haltpoll helps ext4 more than Fxfs (1.31x). Next: attribute the host's ~300 us per fsync inside extentd (request duration histograms by kind, or a native benchmark of WRITE_COMMIT under 4 writers), and cut hand-offs in the request path (complete virtio requests from the client's receiving thread).
<!-- SECTION:NOTES:END -->
