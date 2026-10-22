---
id: TASK-92
title: Performance issues found by the 2026-10-21 review of main
status: To Do
assignee: []
created_date: '2026-10-22 21:06'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
priority: high
type: enhancement
ordinal: 275000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The review of main at 6a0a3f8 (docs/reviews/2026-10-21-main.md, 'Performance issues') confirmed 43 performance issues, each checked by two independent skeptics. 40 are subtasks here (8 high, 32 medium); of the rest, close_now's framing under the writer's locks is TASK-41.1, the window unmaps waiting for the VMM is TASK-79.9's, and the lineage mutex across record fsyncs was fixed on main (7eac693). The report has each one's evidence and a suggested fix.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every subtask is Done, or closed with a measurement showing it costs nothing worth fixing
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
