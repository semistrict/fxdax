---
id: TASK-44.13
title: >-
  Hangs diagnose themselves: stacks, tokio task dumps and the guest's blocked
  tasks captured at a timeout
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
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
- [ ] #1 The guest test runner, at a test's timeout, saves every host process's thread stacks and the guest's blocked tasks (sysrq) before killing it
- [ ] #2 extentd and fxdaxctl dump their tokio tasks and threads on a signal
- [ ] #3 A deliberate deadlock in a test produces the cycle's stacks in its results
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
