---
id: TASK-79.47
title: >-
  The follower simulation records a held object only after the fill ends: false
  failures
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-11-03 02:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 235000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A batch between fs.hold and the held.insert keeps the deleted object, then compares records without excluding it, and fails a correct follower.

Where: crates/follower_sim/src/sim.rs:458, crates/follower_sim/src/sim.rs:463, crates/follower_sim/src/sim.rs:296.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.1, TASK-42.4. No task covers follower_sim recording the held object after the fill. TASK-42.1 built the simulation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
A holder records what it holds under the fill barrier it found it under. No deterministic seam: the batch would have to come between the two; follower_sim's suites pass on macOS and GCE r273.

Fixed in 23923f84d; the simulation's suites pass on r292. AC1 open: its false failures came by schedule, and none was kept as a test.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A holder in follower_sim records what it holds under the fill barrier it found it under (23923f84d); the suites pass on macOS and GCE r292. Decided not to: a failing-first test, as the false failure needed a batch landing between two adjacent steps, which the simulation has no seam to force.
<!-- SECTION:FINAL_SUMMARY:END -->
