---
id: TASK-160
title: 'Structure review of 2026-11-03: what each area''s shape lets bugs through'
status: Done
assignee: []
created_date: '2026-11-03 13:07'
updated_date: '2026-11-08 04:28'
labels:
  - structure
dependencies: []
priority: high
type: task
ordinal: 571000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Five read-only reviews (guest DAX and VFS; host daemon and VMM; extent store and object storage; the Fxfs fork and its glue; tests, sim and tooling) looked for state kept in several places, facts with several owners, protocols spread out, special cases another shape removes. Each finding is a subtask, with its evidence and the past bugs it explains. Ranked within each area by bugs prevented per unit of work. Those already tracked (TASK-153.*, TASK-88, TASK-90, TASK-41.1, TASK-141, TASK-149.*) were left out.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each subtask done, or closed saying why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05: TASK-160.11, 160.23, 160.24, 160.25, 160.26, 160.29, 160.30, 160.31 and 160.32 done on main (GCE r691 and r690, CI 42137709782, macOS workspace 2118 of 2118 at 752515020). AC #1 open: TASK-160.7, 160.13, 160.17, 160.18 and 160.19 are To Do.

2026-11-08 closure sweep: every subtask is Done (TASK-160.1 to 160.32; 160.7, 160.13, 160.17, 160.18 and 160.19, open at the last note, closed since). DoD #1 rests on green main CI 42442410406 at 7de89a88d (macos (workspace), test (workspace), guest-sim, shuttle, sim all success), which contains every subtask's commits.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed: the structure review's 32 findings are each done or closed saying why (backlog: TASK-160.1 to TASK-160.32 all Done). Verified by the subtasks' own evidence and green main CI 42442410406 (7de89a88d), whose macOS and Linux workspace jobs contain all of their commits.
<!-- SECTION:FINAL_SUMMARY:END -->
