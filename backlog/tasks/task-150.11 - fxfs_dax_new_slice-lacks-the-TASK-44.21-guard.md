---
id: TASK-150.11
title: fxfs_dax_new_slice lacks the TASK-44.21 guard
status: To Do
assignee: []
created_date: '2026-11-01 16:52'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: medium
type: bug
ordinal: 526000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit: the missing guard is traced, the interleaving only plausible. dax.c:2411 and :2437; the guard is at fxfs_dax_move, :2344. A sealed piece of a still-writable region can reach fxfs_dax_reach outside the gate and send a writable MAP, racing a seal that shrinks the region: a MAP into space given back.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [ ] #2 Fixed, the test red again with the fix taken out
- [ ] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [ ] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
