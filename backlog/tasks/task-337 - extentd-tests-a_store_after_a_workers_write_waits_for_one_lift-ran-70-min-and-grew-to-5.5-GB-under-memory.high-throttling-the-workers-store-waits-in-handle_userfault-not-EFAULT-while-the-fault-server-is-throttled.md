---
id: TASK-337
title: >-
  extentd::tests::a_store_after_a_workers_write_waits_for_one_lift ran 70+ min
  and grew to 5.5 GB under memory.high throttling: the worker's store waits in
  handle_userfault, not EFAULT, while the fault server is throttled
status: To Do
assignee: []
created_date: '2026-11-10 23:39'
updated_date: '2026-11-10 23:42'
labels:
  - testing
  - dax
dependencies: []
priority: medium
type: bug
ordinal: 738000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-330.1's Linux suite on ubu1 (2026-11-10, branch t330-land, fxjob scope t330-suite at its 12 GiB cap; fxdax.slice at 16.7-17.2 GB past its 16 GiB MemoryHigh, with another agent's 10 GB measurement beside it, 13 M 'high' events). The test hung past 3,240 s (nextest SLOW), every thread parked but one: the worker the loop spawns (kernel_store) in state R, 500 s of CPU, its kernel stack in handle_userfault <- shmem_get_folio_gfp <- shmem_fault <- do_shared_fault; the window's fault server (UffdSecondStage::serve_faults) in __mem_cgroup_handle_over_high <- exc_page_fault, throttled on its way back from a fault; the test thread in JoinHandle::join on the worker. The test process's RSS went from 22 MB (47 min) to 5.48 GB (70 min), then stayed; cargo-nextest held 1.7 GB, presumably the test's captured output. The test expects the worker's store to fail EFAULT at once; it waited instead, for as long as the server stayed throttled. Questions: is a worker's (non-guest thread's) kernel store meant to depend on the server running at all? What grew to 5.5 GB? Does a VM process behave so when its daemon or itself is throttled (a sandbox host at its memory limit)? Not reproduced alone yet.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Related: TASK-193 (the same test flaky on GCE: the worker's write went through). The hung process was killed (SIGKILL) to let the suite go on; its run counts as a failure of the suite, rerun alone after.

Rerun alone on ubu1 once the slice was under its high mark: PASS in 7.7 s. So the hang needs the fault server throttled; what the 5.5 GB were is not known.
<!-- SECTION:NOTES:END -->
