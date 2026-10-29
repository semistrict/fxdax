---
id: TASK-131.4
title: 'Refused guest writes (~4,000 per pgbench VM): what they cost'
status: To Do
assignee: []
created_date: '2026-10-29 21:13'
labels:
  - performance
dependencies: []
parent_task_id: TASK-131
priority: high
ordinal: 475000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox team round 8: ~4,000 refused guest writes per pgbench VM; do they cost anything? TASK-44.12 (2026-10-29) found every refused store a seal race, by KVM's asynchronous page faults' kernel workers (r244: all 133 by kworker threads), each to a page of an extent sealed, which the guest's task retries through its own fault once the guest's seal made its mapping read-only; only a few stall a vCPU (r242: 191 refusals, 14 vCPU EFAULT runs, one failure each). Measure their cost per refusal and per run: the uffd round trip, the restriction (mprotect), the asynchronous fault's worker, the guest task's wait.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Their cost per refusal and per pgbench run measured, and reported to the sandbox team
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
