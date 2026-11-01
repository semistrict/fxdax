---
id: TASK-148
title: >-
  extentd lets a healthy VM process go: its watchdog times a backlog of stage
  operations, not silence
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 05:56'
updated_date: '2026-11-01 06:55'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 501000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs-guest-checks-dax ends about 1 run in 6 to 1 in 3 with extentd killing the VM: 'left a stage operation unacknowledged for 5s'. Measured on 2026-11-01: no stage operation took over 240 ms, and in the run killed the VM process was acknowledging 10,900 operations a second, 659107 to 691771, with some 46,000 queued. The watchdog waits for the newest operation it found, so any backlog of over 5 s of work reads as a hang. Healthy runs reached 4.0 s. Happens on clean main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A VM process that keeps acknowledging stage operations isn't let go while it works through a backlog that takes over 5 s
- [x] #2 A VM process that acknowledges nothing for 5 s is let go, as before
- [x] #3 A VM process acknowledging too slowly to clear a backlog in 60 s is let go, so a trickle holds nothing for ever
- [x] #4 fxfs-guest-checks-dax runs with no VM let go
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Time every stage operation, and log the watchdog's view, on the full checks.
2. Count silence, not the time to clear a backlog, with a cap on a backlog so a trickle holds nothing for ever.
3. Tests of Acks first, then ten full runs with a probe at the old 5 s limit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: Acks::unanswered waited for the newest operation it had found, so a burst taking over 5 s to work through read as silence. Measured: no operation over 240 ms; the VM killed was acknowledging 10,900 a second, some 46,000 behind. Fix: unanswered counts time since acked last moved, and behind caps a backlog at 60 s. a_backlog_worked_through_is_answered and a_backlog_counts_as_behind_until_it_is_cleared failed before the fix. frozen_vm_processes_are_killed still takes about 6 s. Ten full fxfs-guest-checks-dax runs with the fix, FXDAX_EXTENTD_REMOTE exported for the 360 s limit: 10 OK, none let go, and run 9 was 6.0 s behind with 0 ns quiet, which the old watchdog would have killed. scripts/test.sh passes on macOS (1887) and Linux (2029), upstream-changes --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's watchdog no longer kills healthy VM processes: it counts time with nothing acknowledged, 5 s as before, and caps a backlog at 60 s, rather than timing how long a backlog takes. Verified by two new tests that failed first, the frozen VM tests unchanged, and ten full guest check runs, one of which the old watchdog would have killed.
<!-- SECTION:FINAL_SUMMARY:END -->
