---
id: TASK-131.4
title: 'Refused guest writes (~4,000 per pgbench VM): what they cost'
status: In Progress
assignee: []
created_date: '2026-10-29 21:13'
updated_date: '2026-10-29 21:53'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29, first numbers, b5d3246 pgbench-dax on GCE (r264, 5 runs, ~330 s): 103 refusals over the test (the sandbox team saw ~4,000 a VM); the daemon's time on every fault, refused or not, 89 ms for 309 (fault-ns), at most 0.29 ms each with pager waits, against 80 s of MAPs; the VMM's fault thread ('dax faults') under 10 ms of CPU over the test, the sampler's resolution. So the host side of refusals costs under 0.1 s a run, some 0.03%. Open: the guest side (the task's retry through its own fault and MAP, the async page fault's worker), and a run on main whose refusals approach the team's count (FXDAX_PGBENCH_VMS, 8 GiB) to scale it.
<!-- SECTION:NOTES:END -->
