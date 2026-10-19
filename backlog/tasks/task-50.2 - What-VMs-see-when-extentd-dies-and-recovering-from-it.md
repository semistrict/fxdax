---
id: TASK-50.2
title: 'What VMs see when extentd dies, and recovering from it'
status: To Do
assignee: []
created_date: '2026-10-19 20:10'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 151000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
One extentd serves every VM on a host. When its connection drops, a VM's client fails every pending and later request and never reconnects; what the guest's Fxfs does with those errors is untested; a vCPU faulting on a DAX page may block for ever; per-tenant grants do not survive a restart; and the daemon aborts when its memory runs out, which takes every VM on the host with it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 doc-4 or a doc of its own says what a VM sees in each state: daemon gone, restarting, back
- [ ] #2 A test kills extentd under load and checks that each VM either reconnects to the restarted daemon or fails within a bounded time, visibly to its host process
- [ ] #3 No vCPU blocks for ever on a DAX fault after the daemon dies
- [ ] #4 Grants survive a daemon restart, or are re-made by a documented call
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
