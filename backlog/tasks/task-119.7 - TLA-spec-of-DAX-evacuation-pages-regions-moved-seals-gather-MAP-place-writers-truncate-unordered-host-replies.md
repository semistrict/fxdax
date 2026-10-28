---
id: TASK-119.7
title: >-
  TLA+ spec of DAX evacuation: pages, regions, moved, seals, gather/MAP/place,
  writers, truncate, unordered host replies
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 443000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
specs/dax models one page and specs/dax-region one region; evacuation, prune and unordered host replies are unmodelled.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 specs/dax-evacuate model-checks in make check-tla
- [ ] #2 A mutation per rule breaks an invariant
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
