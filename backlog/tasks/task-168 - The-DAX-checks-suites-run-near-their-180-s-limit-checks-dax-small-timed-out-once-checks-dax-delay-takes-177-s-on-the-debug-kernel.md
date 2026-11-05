---
id: TASK-168
title: >-
  The DAX checks suites run near their 180 s limit: checks-dax-small timed out
  once, checks-dax-delay takes 177 s on the debug kernel
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-04 09:18'
updated_date: '2026-11-05 03:15'
labels: []
dependencies: []
priority: medium
type: task
ordinal: 621000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r533 (batch 2026-11-04 at 9e92e6925): fxfs-guest-checks-dax-small timed out at 180 s, in enospc, its guest reading and reclaiming window ranges (fxfs_dax_read_iter, reclaim_range, device_unmap) after window_held_killed passed at 120 s, where main's r532 passed it at 102 s. Four rounds since, two with TASK-160.22 (r535) and two without (r536), all pass: not reproduced, a suite near its limit. The debug kernel's checks-dax-delay took 177 of 180 s (TASK-44.8). A timeout that a slower host or one more check crosses fails runs for nothing, and hides a real slowdown among them. Options: split the checks into suites of their own (TASK-160.30, each check in a process of its own), or time each check rather than the suite, with a budget per check; record each check's time in the results so a slowdown shows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No checks suite runs within 20% of its limit on GCE n2-standard-4, release and debug kernels
- [ ] #2 Each check's time is in the results
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One declarative table of the checks (name, suites, budget, body) that host and guest share (X-macro), replacing the guest's hand-ordered list.
2. The guest times each check (PASS/FAIL: name (took of budget) in the log) and a watchdog ends the run, naming the check and dumping the guest's stacks, when one outruns its budget.
3. The host derives the suite's limit from its checks' budgets, scaled by the guest kernel's kind (FXDAX_GUEST_KERNEL_BUILD, exported by test-linux-host.sh), object storage and FXDAX_CHECKS_SCALE; unknown FXDAX_CHECKS names fail.
4. The runner prints each test's time of its limit; fxfs-guest-checks-overrun tests the watchdog.
5. GCE: measure per-check times on release and debug kernels, set budgets from them, re-run both to verify no suite within 20% of its limit.
<!-- SECTION:PLAN:END -->
