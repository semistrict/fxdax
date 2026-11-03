---
id: TASK-160.18
title: 'extent_store: an extent''s life in one lookup'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 589000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An extent's state is spread over State.extents, Uncommitted.open, KeptExtents, Spent and Sealed; each reader works it out itself (entry_for, check_opens, the write finish, free's branching). Explains TASK-79.1, 79.5, 79.6, 150.6, 150.7. Proposal: one Life lookup and transitions as State methods. Hot path: prototype and measure first.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Prototyped and measured
- [ ] #2 Decided
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
