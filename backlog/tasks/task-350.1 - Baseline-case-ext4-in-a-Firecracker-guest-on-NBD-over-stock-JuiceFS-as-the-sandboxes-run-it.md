---
id: TASK-350.1
title: >-
  Baseline case: ext4 in a Firecracker guest on a loop device over a local image
  file
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 10:55'
updated_date: '2026-11-11 12:29'
labels: []
dependencies: []
parent_task_id: TASK-350
priority: medium
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every scenario under TASK-350 compares against this (user, 2026-11-11: no JuiceFS; local files, loop-mounted). A raw image file on ubu1's local ZFS, attached with losetup, the loop device given to Firecracker as a virtio-blk drive, ext4 in the guest. Guest RAM, vCPUs and kernel match the fxdax case's. The existing ext4-guest-kernel case (guest/tests/test_cases/src/test_fs_bench.rs, Backend::Ext4GuestKernel) gives Firecracker an image file directly; this one goes through a loop device. Loop devices need root, as the e2e suite's loop-mount tests do.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A bench case (e.g. ext4-loop) boots a Firecracker guest with ext4 on a virtio-blk drive backed by a loop device over a local image file, and runs the existing fs-bench and pgbench workloads on ubu1
- [ ] #2 The drive runs with Firecracker cache_type Writeback (guest flushes reach the file as fsync) and, as a variant, Unsafe; the loop device's direct-I/O setting, the backing filesystem and the cache mode are recorded with each run
- [ ] #3 The case tears down its loop device and image whatever the outcome
- [ ] #4 A test in the repo checks the case: a file fsync'd in the guest is read back after the VM restarts on the same loop device
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crate test_cases: loop_device.rs attaches a free loop device over an image (LOOP_CONFIGURE, LO_FLAGS_AUTOCLEAR; buffered or direct I/O), so it detaches itself once the test process and Firecracker close it, whatever the outcome.
2. Vm::loop_disk: a virtio-blk disk on such a device, held by the Vm/Firecracker; says device, dio, backing fs and cache_type on stderr.
3. Cases: bench-fs-ext4-loop (Writeback), bench-fs-ext4-loop-unsafe; pgbench-ext4-loop-writeback, pgbench-ext4-loop-unsafe.
4. ext4-loop-persists: guest fsyncs a file and resets unsynced; a second VM on the same loop device reads it whole; the host checks the device detached.
5. Prove on ubu1 as root under fxjob: ext4-loop-persists, one iteration of bench-fs-ext4-loop and pgbench-ext4-loop-writeback.
<!-- SECTION:PLAN:END -->
