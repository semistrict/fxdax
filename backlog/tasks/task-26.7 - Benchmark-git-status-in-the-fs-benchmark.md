---
id: TASK-26.7
title: 'Benchmark: git status in the fs benchmark'
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
updated_date: '2026-11-06 06:51'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
priority: medium
type: task
ordinal: 43000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
git status is one of the most common operations on a development VM's filesystem, and the design (doc-2) is judged by it. The fs benchmark (guest/tests/test_cases/src/test_fs_bench.rs, run by scripts/bench.sh and bench-gce.sh) has no git workload: its groups are io, metadata, build, sqlite and pgbench. Its backends (test_fs_bench.rs:59-95) are ext4 on virtio-blk, ext4 on virtio-pmem with and without DAX, Fxfs in the guest with and without DAX, and their fork and followed cases; virtio-fs, which the first baseline was to compare, went with libkrun (5ba394977, TASK-40). doc-10's shared checkouts time a first git status already (guest/tests/test_cases/src/test_fxfs_guest_shared_volumes.rs), on a fork of a published base only.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The benchmark times git status on a repository of several thousand files, cold (after dropping the guest's caches) and warm, on every backend
- [ ] #2 A GCE run records the baseline for ext4 on virtio-blk, ext4-dax-pmem and Fxfs with and without DAX in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: the benchmark's path (vendor/libkrun/tests/...) and its passthrough baseline went with libkrun (5ba394977, TASK-40); description and AC #2 brought to guest/tests and today's backends.
<!-- SECTION:NOTES:END -->
