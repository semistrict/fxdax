---
id: TASK-25
title: Benchmark fxdax against ext4 on a virtio-blk device in the guest; within 2x
status: To Do
assignee: []
created_date: '2026-10-10 14:53'
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
