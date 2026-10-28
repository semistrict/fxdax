---
id: TASK-112.3
title: >-
  Design: whether a guest fsync needs a seal, and whether the seal's window MAPs
  can leave the commit path
status: To Do
assignee: []
created_date: '2026-10-28 02:30'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: medium
ordinal: 426000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Sandbox round 7b (TASK-112): DAX seals cost 0.54 ms against 0.32 without, about three window MAPs each (1.8 evacuation, ~1 restage) at ~90 us. They ask: could fsync commit an open extent durably without write-protecting and evacuating (sealing only at fork, snapshot or window reclaim); or could evacuation and restage run after the commit replies. Related: TASK-66 (fsync durability contract), TASK-109.1 (evacuation remaps), TASK-109.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The answer to both questions, with the durability and sharing constraints behind it, is recorded in a decision or doc
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
