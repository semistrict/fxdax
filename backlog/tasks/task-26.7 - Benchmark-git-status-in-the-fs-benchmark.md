---
id: TASK-26.7
title: 'Benchmark: git status in the fs benchmark'
status: To Do
assignee: []
created_date: '2026-10-13 19:54'
labels: []
milestone: m-3
dependencies: []
documentation:
  - >-
    backlog/docs/doc-2 -
    Guest-owned-metadata-Fxfs-in-the-guest-kernel-over-a-virtio-extent-store.md
parent_task_id: TASK-26
type: task
ordinal: 43000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
git status is one of the most common operations on a development VM's filesystem, and the design (doc-2) is judged by it. The fs benchmark (vendor/libkrun/tests/test_cases/src/test_fs_bench.rs) has no git workload. Adding it now also gives a baseline for today's virtio-fs fxdax.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The benchmark times git status on a repository of several thousand files, cold (after dropping the guest's caches) and warm, on every backend
- [ ] #2 A GCE run records the baseline for ext4, fxdax and passthrough in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
