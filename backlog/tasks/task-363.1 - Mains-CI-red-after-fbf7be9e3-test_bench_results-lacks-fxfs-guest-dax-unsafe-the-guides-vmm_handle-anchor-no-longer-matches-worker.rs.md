---
id: TASK-363.1
title: >-
  Main's CI red after fbf7be9e3: test_bench_results lacks fxfs-guest-dax-unsafe;
  the guide's vmm_handle anchor no longer matches worker.rs
status: To Do
assignee: []
created_date: '2026-11-11 17:59'
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
- [ ] #1 test_bench_results passes with fxfs-guest-dax-unsafe named, in ORDER too
- [ ] #2 The guide's vmm_handle anchor matches worker.rs again; main's CI green
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
