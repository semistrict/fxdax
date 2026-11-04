---
id: TASK-44.13
title: >-
  Hangs diagnose themselves: stacks, tokio task dumps and the guest's blocked
  tasks captured at a timeout
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-04 10:40'
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
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Committed with 44.8: the runner saves eu-stack stacks of a timed-out test's processes; the guest reports tasks blocked 10 s. runner-deadlock (FXDAX_RUNNER_DEADLOCK=1), GCE r81: TIMEOUT, stacks-<pid>.txt naming holds_first_then_second and holds_second_then_first. AC2 left: tokio task dumps of extentd/fxdaxctl on a signal.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd and fxdaxctl write their threads' stacks on SIGQUIT (fxdax::stacks::on_sigquit, 0dd72abd8), now tested against the real processes (the_daemon_writes_its_stacks_on_sigquit, fxdaxctl_writes_its_stacks_on_sigquit). Not done: tokio's task dump, which needs its unstable cfg. Verified: macOS scripts/test.sh on the tree merged with batch 2026-11-04 (workspace 2,083/2,083, Shuttle, scripts); GitHub CI on the branch, every job (run 41806511048, Linux workspace with the codec's tests); GCE r540 in the jailer under the production seccomp filter (fxfs-guest-dax, checks-dax, crash-dax, migrate-tenant-dax, pgbench pass; the refusal gate's failure is TASK-167's, the jailer never mounting /proc).
<!-- SECTION:FINAL_SUMMARY:END -->
