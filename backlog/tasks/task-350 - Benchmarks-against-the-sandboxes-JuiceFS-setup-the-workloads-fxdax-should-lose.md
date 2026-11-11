---
id: TASK-350
title: >-
  Benchmarks against the sandboxes' JuiceFS setup: the workloads fxdax should
  lose
status: To Do
assignee: []
created_date: '2026-11-11 10:55'
updated_date: '2026-11-11 10:56'
labels: []
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax replaces the sandboxes' current storage: each VM's rootfs is a file in stock JuiceFS (v1.3.1, vendored in juicebox's third_party/juicefs; production runs it vanilla), opened through the JuiceFS Go library inside the orchestrator and served to the VM over NBD, with ext4 in the guest (juicebox packages/orchestrator/pkg/sandbox/jfs, rootfs/jfs_nbd.go). Its setup: 4 MiB blocks, no compression, no writeback, MaxUpload 20, a local disk cache of whole blocks (JUICEFS_CACHE_SIZE_MB, 10 GiB by default), metadata in PostgreSQL, data in a GCS bucket; clones are JuiceFS metadata clones. Its NBD server neither advertises nor accepts FLUSH, so a guest fsync ends in JuiceFS's memory buffer.

Where JuiceFS is worse (random small writes, durability) the user has measured. This task is the opposite: the workloads where fxdax is expected to lose to that setup, each measured against it on the same host, so that each loss is known in size before the sandboxes switch. Analysis that picked them (2026-11-11): the daemon's memory is a DAX guest's only file cache and a miss decompresses a whole 1 MiB part (crates/fxdax/src/cache.rs); cold DAX scans ran 7-9x below ext4 on virtio-blk (docs/measurements/gce-fs-bench-2026-10-31-refresh); pgbench 0.55x ext4 with flushes dropped (docs/measurements/gce-pgbench-baselines-2026-11-08); syncfs waits for the bucket (~80 ms on GCS, docs/measurements/gcs-round-trips-2026-11-08.md); Fxfs metadata ops 1.5-2x ext4; the bucket never restores file order (doc-7, Garbage collection, "Not yet").

Runs on ubu1 through fxjob --bench, never GCP; the bucket a local OSI-licensed S3 server (scripts/test-rustfs.sh's RustFS), the same one for both systems, with the same added latency.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A JuiceFS-over-NBD baseline case exists (subtask) and every scenario subtask has run against it and against fxdax
- [ ] #2 One measurement document under docs/measurements tables each scenario: fxdax, JuiceFS, ratio, run IDs, and what each system was configured with
- [ ] #3 Each loss larger than 1.5x has a backlog task of its own for the fix or a recorded decision to accept it
- [ ] #4 Every measured run isolates itself on shared ubu1: fxjob --bench (cores 4-7 exclusive, other jobs held to 0-3), started only at 1-min load < 16 and the load recorded before and after; fxdax and JuiceFS rounds interleaved (A B A B ...), at least 3 each, medians and ranges reported; both systems' caches and stores on the same ext4 scratch disk (/mnt/fxdax-scratch, not ZFS /home, whose ARC bypasses the page cache), host page cache dropped before each cold round; a run whose load passed 16 or whose spread exceeds 15% is rerun, not reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
