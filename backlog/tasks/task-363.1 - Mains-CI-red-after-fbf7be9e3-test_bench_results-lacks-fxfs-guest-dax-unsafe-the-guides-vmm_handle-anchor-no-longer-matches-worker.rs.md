---
id: TASK-363.1
title: >-
  Main's CI red after fbf7be9e3: test_bench_results lacks fxfs-guest-dax-unsafe;
  the guide's vmm_handle anchor no longer matches worker.rs
status: Done
assignee: []
created_date: '2026-11-11 17:59'
updated_date: '2026-11-11 19:41'
labels:
  - bug
  - ci
dependencies: []
parent_task_id: TASK-363
priority: high
ordinal: 747000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Main CI 43069997544 at fbf7be9e3 (TASK-363's extent devices with cache_type Unsafe): (1) scripts' test_bench_results.Registry.test_every_filesystem_the_cases_measure_is_named fails: the cases now measure fxfs-guest-dax-unsafe, which neither the test's list nor scripts/bench_results.py's ORDER names; (2) the guide job fails: 'anchor vmm_handle: its snippet occurs 0 times in vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs, not once: pub(super) async fn handle(store: &RemoteExte...' (guide/tools/check.ts), the function's signature having changed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 test_bench_results passes with fxfs-guest-dax-unsafe named, in ORDER too
- [x] #2 The guide's vmm_handle anchor matches worker.rs again; main's CI green
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed in cf8fe5a62. The firecracker-forward job's failure had a third cause, worse than the two in the description: 07c1d9c16 (a backlog commit, TASK-344's) put vendor/firecracker back from TASK-363's b8b1a9b07 to r6's 323b5a5, so main ran without TASK-363's Unsafe cache in the VMM. vendor/firecracker is back at b8b1a9b07. bench_results names fxfs-guest-dax-unsafe in ORDER, in the test's list, and with ext4-loop-unsafe then fxfs-guest-dax as its baselines. The vmm_handle anchor is 'pub(super) async fn handle(', since its parameters now span lines. Checked: scripts' tests 289 OK, guide check 62 anchors matching, firecracker-forward against fbf7be9e3 and origin/main. b8b1a9b07 is on the fork's fxdax-v1.16.1-extent-unsafe branch, not yet under a fxdax-v1.16.1-rN tag: that is TASK-363's.
<!-- SECTION:NOTES:END -->
