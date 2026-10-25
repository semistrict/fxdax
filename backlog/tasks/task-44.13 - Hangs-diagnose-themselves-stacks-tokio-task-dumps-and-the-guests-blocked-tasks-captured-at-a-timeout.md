---
id: TASK-44.13
title: >-
  Hangs diagnose themselves: stacks, tokio task dumps and the guest's blocked
  tasks captured at a timeout
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-24 23:57'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 392000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-98's import deadlock took the sandbox team installing gdb on a box to see; the runner's TIMEOUT kills a test and keeps nothing of why. All threads' stacks, tokio's task dump and the guest's sysrq-w/t at the moment of a hang name the cycle.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The guest test runner, at a test's timeout, saves every host process's thread stacks and the guest's blocked tasks (sysrq) before killing it
- [ ] #2 extentd and fxdaxctl dump their tokio tasks and threads on a signal
- [x] #3 A deliberate deadlock in a test produces the cycle's stacks in its results
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Committed with 44.8: the runner saves eu-stack stacks of a timed-out test's processes; the guest reports tasks blocked 10 s. runner-deadlock (FXDAX_RUNNER_DEADLOCK=1), GCE r81: TIMEOUT, stacks-<pid>.txt naming holds_first_then_second and holds_second_then_first. AC2 left: tokio task dumps of extentd/fxdaxctl on a signal.
<!-- SECTION:NOTES:END -->
