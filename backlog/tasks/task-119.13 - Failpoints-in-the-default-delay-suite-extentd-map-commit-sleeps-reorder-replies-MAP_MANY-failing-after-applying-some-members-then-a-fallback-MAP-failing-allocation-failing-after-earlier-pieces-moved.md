---
id: TASK-119.13
title: >-
  Failpoints in the default delay suite: extentd map/commit sleeps reorder
  replies; MAP_MANY failing after applying some members, then a fallback MAP
  failing; allocation failing after earlier pieces moved
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 451000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra/Fable: FXDAX_FAILPOINTS exists but no default run uses it; dax.c publishes pieces incrementally (map_extents) and TASK-79.106 has no deterministic test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Partial-failure paths each reached by a check that verifies contents after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
