---
id: TASK-299
title: >-
  Flaky: fxfs-guest-checks tracepoints expects an order-0 read fault, gets a PMD
  fault (order 9)
status: To Do
assignee: []
created_date: '2026-11-09 10:02'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 714000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE t257dbg3 (debug kernel, main 6408801bc + TASK-257 + TASK-292, fxdax-keep-n2-standard-4-us-west1-a-1): fxfs-guest-checks-dax-delay round 2 (seed 1792699608765578114) and fxfs-guest-checks-dax-small round 4 (seed 1792807242380890800) failed 'FAILED: tracepoints': tracepoints.rs (the load's read fault): assert_eq!((fault.number("order"), fault.number("write")), (0, 0)) failed with Event { name: "fxfs_dax_fault_start", fields: {ino: 1036, order: 9, pgoff: 2, write: 0} }: the read fault of page 2 was taken as a PMD fault. The check came with TASK-44.16 (fd37ccbe2). Either the mapping's alignment lets the kernel try a PMD fault (the check should allow order 9, or map so it can't), or a PMD fault there is itself wrong.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The check states what fault orders a read of page 2 may take, and why
- [ ] #2 tracepoints passes 20 rounds of fxfs-guest-checks-dax-delay and -dax-small on the debug kernel
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
