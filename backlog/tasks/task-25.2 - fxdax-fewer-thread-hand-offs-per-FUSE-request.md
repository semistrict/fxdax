---
id: TASK-25.2
title: 'fxdax: fewer thread hand-offs per FUSE request'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 19:08'
updated_date: '2026-10-10 21:32'
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
- [x] #2 Creates and unlinks no slower, and the I/O workloads no slower
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done so far, each measured on GCE (fxdax without DAX): A. jobs spawned straight onto the executor's runtime, and 1 executor thread by default; B. libkrun's fxdax device loop runs on fxdax's executor (Spawner), waiting on the queues' eventfds there, handling requests concurrently; C. lookups and getattrs from Fxfs's nodes, connections opened when first needed (FxDirectory::lookup_child, get_attributes, serve in fxfs-platform), and Fxfs's directory entry cache at 16,384. 3-iteration result (gce-fs-bench-2026-10-10T1939Z against T1823Z): cold stats 2,572 -> 4,806/s (1.87x; single runs reached 5,239), creates 1,184 -> 1,488 (passthrough 1,452), unlinks 2,835 -> 3,537, I/O unchanged. AC #1 (2x cold stats) not met yet. Where the rest goes (gce-fs-bench-2026-10-10T1943Z, per-opcode timing inside fxdax): a LOOKUP takes ~29 µs in fxdax, of ~208 µs a cold stat; ~180 µs is outside fxdax, against ~80 µs for passthrough's whole request. KVM halt polling is on (up to 200 µs). Next to look at: what in the device path or the guest makes fxdax's requests slower outside fxdax than passthrough's.

3 iterations after TASK-25.3 to 25.6 (T2124Z): cold stats 4,958/s without DAX, 1.97x the baseline's 2,518 (AC #1 wants 2x); single runs since reach 5,050 to 5,399. TASK-25.7 found why fxdax's requests cost ~90 µs more than passthrough's outside fxdax: its replies come after the waiting vCPU's halt-poll window, so the vCPU sleeps and is woken each time; making replies quicker (17-22 µs in the device against passthrough's 7-10) is what is left.
<!-- SECTION:NOTES:END -->
