---
id: TASK-44.16
title: Guest DAX tracepoints and a trace checker of the protocol's invariants
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-28 01:26'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: low
ordinal: 395000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's beliefs (regions open, pages writable) and the host's MAPs are each logged in part; nothing checks them against each other over a run. Tracepoints at region open, MAP sent and returned, seal and evacuation, merged with extentd's MAP log, let an offline checker replay the protocol's state machine (or the kernel's RV monitors run it live) and name the first step that breaks an invariant.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The guest emits tracepoints at the DAX protocol's steps, and extentd logs its MAPs with the same offsets
- [ ] #2 A checker replays a run's merged trace against the protocol's invariants and reports the first violation
- [ ] #3 TASK-99's run, replayed, is reported
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Sandbox team round 7 (TASK-111) asks for tracepoints or USDT probes at seal start and end, MAP by reason, evacuation, guest fault to extentd, and window reclaim, so bpftrace can measure latencies.
<!-- SECTION:NOTES:END -->
