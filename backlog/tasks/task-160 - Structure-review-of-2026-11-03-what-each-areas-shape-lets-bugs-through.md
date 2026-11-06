---
id: TASK-160
title: 'Structure review of 2026-11-03: what each area''s shape lets bugs through'
status: To Do
assignee: []
created_date: '2026-11-03 13:07'
updated_date: '2026-11-06 02:22'
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
- [ ] #1 Each subtask done, or closed saying why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05: TASK-160.11, 160.23, 160.24, 160.25, 160.26, 160.29, 160.30, 160.31 and 160.32 done on main (GCE r691 and r690, CI 42137709782, macOS workspace 2118 of 2118 at 752515020). AC #1 open: TASK-160.7, 160.13, 160.17, 160.18 and 160.19 are To Do.
<!-- SECTION:NOTES:END -->
