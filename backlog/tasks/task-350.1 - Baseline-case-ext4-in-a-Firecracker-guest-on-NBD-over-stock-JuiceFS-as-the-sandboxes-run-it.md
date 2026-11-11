---
id: TASK-350.1
title: >-
  Baseline case: ext4 in a Firecracker guest on a disk image file on a stock
  JuiceFS FUSE mount
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 10:55'
updated_date: '2026-11-11 10:59'
labels: []
dependencies: []
parent_task_id: TASK-350
priority: medium
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scenario under TASK-350 compares against this. The user (2026-11-11): the baseline is VM block device files on a FUSE mount of vanilla JuiceFS, not juicebox's in-process library and NBD server. So: the stock juicefs binary (latest release, v1.4.2 on 2026-11-11, Apache-2.0) formatted with 4 MiB blocks and no compression, mounted with its default options (no writeback), its block cache on the host's ext4 scratch disk; PostgreSQL (PostgreSQL License) for metadata and the bucket on the same local RustFS (Apache-2.0) the fxdax case uses, both in containers; an image file on the mount given to Firecracker as a virtio-blk drive, ext4 in the guest. Guest RAM, vCPUs and kernel match the fxdax case's, so the guest's own page cache is what that setup gives it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A bench case (e.g. ext4-juicefs-fuse) boots a Firecracker guest with ext4 on a virtio-blk drive whose image is a file on a stock JuiceFS FUSE mount, and runs the existing fs-bench and pgbench workloads on ubu1
- [ ] #2 The drive runs with Firecracker cache_type Writeback (guest flushes reach JuiceFS as fsync) and, as a variant, Unsafe (flushes dropped); each run records which
- [ ] #3 The JuiceFS version, format and mount options, cache size and location are recorded with each run
- [ ] #4 Bucket latency is the same setting as fxdax's FXDAX_REMOTE_LATENCY_MS (applied to both systems' requests) and metadata latency is configurable (default ~1 ms, a managed PostgreSQL's); defaults recorded
- [ ] #5 A test in the repo checks the case: a file fsync'd in the guest is read back after the VM restarts on the same image, with the JuiceFS mount remounted in between
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
