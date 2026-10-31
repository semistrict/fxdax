---
id: TASK-141.2
title: 'Branches in a bucket say their format, and a reader refuses a newer one'
status: To Do
assignee: []
created_date: '2026-10-31 20:30'
labels: []
dependencies: []
parent_task_id: TASK-141
ordinal: 489000
---

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each branch's objects carry a format version
- [ ] #2 A test: a daemon refuses a branch of a newer format, naming it, and reads none of it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
