---
id: TASK-160.30
title: Guest checks each in a process of its own
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 601000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
About 90 checks run in PID 1 under catch_unwind: a crash takes the VM, so checks are ordered by hand; state leaks between them (unmap_what_it_left, Param and Pauses guards); an unknown name in FXDAX_CHECKS runs nothing and passes. Explains TASK-119.18, 3a9e74bcd, 2e1a2b225, 9c0f768c9. Proposal: each check a declaration with when it applies and an optional host-side verify; the agent re-runs itself per check with a deadline that dumps stacks; parameters reset in one place; unknown names fail; the 4,900-line file split by area.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A check that SIGBUSes or hangs fails by name, the rest pass
- [ ] #2 FXDAX_CHECKS=nope fails
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
