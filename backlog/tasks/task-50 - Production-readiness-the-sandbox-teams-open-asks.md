---
id: TASK-50
title: 'Production readiness: the sandbox team''s open asks'
status: To Do
assignee: []
created_date: '2026-10-19 20:10'
labels: []
dependencies: []
priority: high
ordinal: 149000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team reviewed fxdax at 8d31ab2 for production (their vendor/fxdax-requirements.md, 'Open asks, in full') and listed seven open items, four of them blockers: running under Firecracker's real seccomp filter and the jailer, what VMs see when extentd dies, the host mapping limit (TASK-12), and the daemon's memory caps. The others: userfaultfd without the node-wide sysctl, store API gaps, and the guest filesystem's lost-fsync bug (TASK-26.6.2) and missing features (TASK-26.19). Each is a subtask or an existing task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every subtask, TASK-12 and TASK-26.6.2 are Done
- [ ] #2 The sandbox team's open asks each have an answer in the repository: done, tested, or explicitly unsupported with what programs see
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
