---
id: TASK-350
title: >-
  Benchmarks against local disk images on loop devices: the workloads fxdax
  should lose
status: To Do
assignee: []
created_date: '2026-11-11 10:55'
updated_date: '2026-11-11 13:47'
labels: []
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxdax replaces VM disks kept as image files on JuiceFS. Selection rule (user, 2026-11-11): only workloads FAST on production JuiceFS and SLOW on fxdax; JuiceFS's known weaknesses (small random writes, uncached bucket reads) are out of scope. Production runs Firecracker with cache_type Unsafe (user, 2026-11-11): guest flushes, fsync, sync, syncfs and freeze never leave the host there.

Baseline (user, 2026-11-11): JuiceFS with everything in its local cache, stood in for by a local image file on ubu1's ZFS, attached as a loop device and given to Firecracker as a virtio-blk drive with cache_type Unsafe, ext4 in the guest (bench-fs-ext4-loop-unsafe, pgbench-ext4-loop-unsafe; TASK-350.1). Dropped: fxdax with data only in the bucket (JuiceFS also slow there), long-overwritten files (TASK-350.7: JuiceFS fragments too).

Analysis that picked the scenarios (2026-11-11): the daemon's memory is a DAX guest's only file cache and a miss decompresses a whole 1 MiB part (crates/fxdax/src/cache.rs); cold DAX scans ran 7-9x below ext4 on virtio-blk (docs/measurements/gce-fs-bench-2026-10-31-refresh); pgbench 0.55x ext4 with flushes dropped (docs/measurements/gce-pgbench-baselines-2026-11-08); syncfs waits for the bucket (~80 ms on GCS, docs/measurements/gcs-round-trips-2026-11-08.md); Fxfs metadata ops 1.5-2x ext4.

Runs on ubu1, never GCP. fxdax's bucket is a local OSI-licensed S3 server (scripts/test-rustfs.sh's RustFS) with FXDAX_REMOTE_LATENCY_MS recorded.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A loop-device baseline case exists (TASK-350.1) and every scenario subtask has run against it and against fxdax
- [ ] #2 One measurement document under docs/measurements tables each scenario: fxdax, baseline, ratio, run IDs, and how each was configured
- [ ] #3 Each loss larger than 1.5x has a backlog task of its own for the fix or a recorded decision to accept it
- [ ] #4 Every measured run isolates itself on shared ubu1: fxjob --bench (cores 4-7 exclusive, other jobs held to 0-3), started only at 1-min load < 16 and the load recorded before and after; fxdax and baseline rounds interleaved (A B A B ...), at least 3 each, medians and ranges reported; host caches dropped before each cold round (page cache, and the ZFS ARC for the baseline's file, e.g. by exporting/importing or primarycache=metadata, recorded); a run whose load passed 16 or whose spread exceeds 15% is rerun, not reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
