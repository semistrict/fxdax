---
id: TASK-149.21
title: 'The sim''s writers upload inherited extents, as diff does'
status: To Do
assignee: []
created_date: '2026-11-01 19:12'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
type: task
ordinal: 541000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-150.1's blind spot: extent_remote's simulation seals only IDs its writers minted, so diff re-uploading inherited extents never met a collector there; only the fork sweep's Mishap::Collected reaches it now. Beside TASK-88.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sim runs include children uploading inherited extents
- [ ] #2 TASK-150.1's bug, put back, fails the sim
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
