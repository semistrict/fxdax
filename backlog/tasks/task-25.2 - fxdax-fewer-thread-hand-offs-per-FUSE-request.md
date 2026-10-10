---
id: TASK-25.2
title: 'fxdax: fewer thread hand-offs per FUSE request'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 19:11'
labels: []
dependencies: []
references:
  - docs/measurements/gce-fs-bench-2026-10-10T1823Z.md
parent_task_id: TASK-25
type: feature
ordinal: 27000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A cold stat takes ~400 µs on fxdax against ~110 µs on libkrun's virtio-fs passthrough on GCE (docs/measurements/gce-fs-bench-2026-10-10T1843Z). The VMM is mostly idle; its CPU time goes to threads waking one another (profile: gce-fs-bench-2026-10-10T1900Z): each request crosses from libkrun's fs worker to fxdax's executor (a receive loop that spawns each job as a task, on one of 4 threads), to tokio workers serving the in-process fuchsia.io connection that lookup opens, and back. With 1 executor thread, cold stats rose 28% and creates 19% (gce-fs-bench-2026-10-10T1903Z). Cut the hand-offs: spawn jobs straight onto the executor, keep Fxfs's work on as few threads as serves it, and serve lookups and attribute reads from Fxfs's nodes rather than through fuchsia.io connections.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cold stats on fxdax without DAX at least twice the 2,518/s of gce-fs-bench-2026-10-10T1843Z, measured on GCE
- [ ] #2 Creates and unlinks no slower, and the I/O workloads no slower
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Measure each step on GCE (fxdax without DAX, metadata and I/O, against gce-fs-bench-2026-10-10T1843Z).
A. Executor: spawn jobs straight onto its tokio runtime from the caller's thread, rather than through a receive loop that spawns them; then choose its thread count from measurement (1 measured +28% stats).
B. Serve virtio-fs requests on fxdax's executor: libkrun's worker hands each descriptor chain over without waiting, and the executor completes it and signals the guest, removing the hop back.
C. Lookups and getattrs from Fxfs's nodes rather than fuchsia.io connections, opening a connection only when an operation needs one.
<!-- SECTION:PLAN:END -->
