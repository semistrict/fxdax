---
id: TASK-350
title: >-
  Benchmarks against VM disk images on stock JuiceFS: the workloads fxdax should
  lose
status: To Do
assignee: []
created_date: '2026-11-11 10:55'
updated_date: '2026-11-11 10:59'
labels: []
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax replaces storage that keeps VM block device files on JuiceFS. The baseline (user, 2026-11-11): stock JuiceFS mounted with FUSE, each VM's disk an image file on the mount, given to Firecracker as a virtio-blk drive, ext4 in the guest; not juicebox's in-process library and NBD server.

Where JuiceFS is worse (random small writes, durability) the user has measured. This task is the opposite: the workloads where fxdax is expected to lose to that setup, each measured against it on the same host, so that each loss is known in size before the sandboxes switch. Analysis that picked them (2026-11-11): the daemon's memory is a DAX guest's only file cache and a miss decompresses a whole 1 MiB part (crates/fxdax/src/cache.rs); cold DAX scans ran 7-9x below ext4 on virtio-blk (docs/measurements/gce-fs-bench-2026-10-31-refresh); pgbench 0.55x ext4 with flushes dropped (docs/measurements/gce-pgbench-baselines-2026-11-08); syncfs waits for the bucket (~80 ms on GCS, docs/measurements/gcs-round-trips-2026-11-08.md); Fxfs metadata ops 1.5-2x ext4; the bucket never restores file order (doc-7, Garbage collection, "Not yet").

Runs on ubu1 through fxjob --bench, never GCP; the bucket a local OSI-licensed S3 server (scripts/test-rustfs.sh's RustFS), the same one for both systems, with the same added latency.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One measurement document under docs/measurements tables each scenario: fxdax, JuiceFS, ratio, run IDs, and what each system was configured with
- [ ] #2 Each loss larger than 1.5x has a backlog task of its own for the fix or a recorded decision to accept it
- [ ] #3 Every measured run isolates itself on shared ubu1: fxjob --bench (cores 4-7 exclusive, other jobs held to 0-3), started only at 1-min load < 16 and the load recorded before and after; fxdax and JuiceFS rounds interleaved (A B A B ...), at least 3 each, medians and ranges reported; both systems' caches and stores on the same ext4 scratch disk (/mnt/fxdax-scratch, not ZFS /home, whose ARC bypasses the page cache), host page cache dropped before each cold round; a run whose load passed 16 or whose spread exceeds 15% is rerun, not reported
- [ ] #4 A baseline case of disk image files on a stock JuiceFS FUSE mount exists (TASK-350.1) and every scenario subtask has run against it and against fxdax
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
