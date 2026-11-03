---
id: TASK-160
title: 'Structure review of 2026-11-03: what each area''s shape lets bugs through'
status: To Do
assignee: []
created_date: '2026-11-03 13:07'
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
