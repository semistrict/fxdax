---
id: TASK-25
title: Benchmark fxdax against ext4 on a virtio-blk device in the guest; within 2x
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 14:53'
updated_date: '2026-10-10 18:41'
labels: []
dependencies: []
type: chore
ordinal: 25000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user wants fxdax's performance measured against the usual alternative: a guest's own filesystem (ext4) on a virtual block device (virtio-blk backed by an image file on the same host filesystem), in the same libkrun VM setup, on macOS (HVF) and Linux (KVM). Target: fxdax within 2x of ext4 on each workload. Workloads should cover sequential and random reads and writes (cold and warm cache), small-file metadata (create, stat, readdir, unlink of many files), fsync-heavy writes, mmap through DAX vs page cache, and a build-like mixed workload (e.g. untar plus compile of a source tree). Use a standard tool where it fits (fio for I/O; a scripted metadata and build workload otherwise), run several iterations, and report medians and spread.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A repeatable benchmark script runs the workloads in libkrun guests on fxdax (with and without DAX) and on ext4 over virtio-blk, on macOS and Linux
- [ ] #2 Results are recorded in the repo (a doc with a table per host) with the setup used
- [ ] #3 Every workload is within 2x of ext4, or the gaps are filed as tasks with their causes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
- [ ] #4 The benchmark script is in the repo and documented
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Run where butter runs its benchmarks: disposable, labelled GCE hosts with nested KVM that delete themselves, with results kept in docs/measurements.
1. Guest workloads: a libkrun test case per filesystem (bench-fs-fxdax-dax, bench-fs-fxdax-nodax, bench-fs-ext4; tests/test_cases/src/test_fs_bench.rs) on a Fedora rootfs built with podman (fio, gcc, make, e2fsprogs, GNU make 4.4.1's source). 2 vCPUs, 2 GiB; 8 GiB sparse images on the host's filesystem; ext4 through krun_add_disk3 with libkrun's recommended defaults. Workloads: fio seq write/read cold/warm (1 GiB), 4 KiB random read cold/warm and write, mmap random read/write, fsynced 4 KiB appends; create/stat/readdir/unlink of 10,000 files; untar+configure+make of GNU make. Timed by wall clock, N iterations, gated on FXDAX_BENCH_ITERATIONS; JSON per case into FXDAX_BENCH_RESULTS.
2. scripts/bench.sh runs the three cases via test-e2e.sh; scripts/bench-report.py tabulates medians, spread and ratios to ext4.
3. scripts/lib/gce.sh: GCE plumbing shared by test-gce.sh and the new bench-gce.sh, after butter (owner labels, max run duration, inactivity lease startup script, remote() renewing it, verified deletion, all always deletes). scripts/lib/linux-host-setup.sh: host provisioning shared by test-linux-host.sh and bench-linux-host.sh (podman added).
4. Run on GCE (c3-standard-8), smoke with 1 iteration then 5; record docs/measurements/gce-fs-bench-<date>.md with setup and table; file tasks for gaps over 2x with causes.
macOS is not benchmarked here: the user asked not to benchmark locally, and there is no remote Mac.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Ran on GCE (c3-standard-8, nested KVM) in 481 s with scripts/bench-gce.sh: docs/measurements/gce-fs-bench-2026-10-10.md. A libkrun virtio-fs passthrough case shows most of the metadata and mmap-read cost is libkrun's virtio-fs transport under nested KVM. fxdax-specific gaps: DAX cold reads (18x), DAX sequential writes (3x) and mmap writes (4x), cold stat (4x passthrough's). Not measured on macOS: the user asked not to benchmark locally. Run time cut from 1.7 h to under 10 min at the user's request: persistent build tree on the host, prebuilt libkrunfw, Alpine rootfs, smaller workloads, 3 iterations.
<!-- SECTION:NOTES:END -->
