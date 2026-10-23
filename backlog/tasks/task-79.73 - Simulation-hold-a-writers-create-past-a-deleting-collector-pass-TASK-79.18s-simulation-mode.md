---
id: TASK-79.73
title: >-
  Simulation: hold a writer's create past a deleting collector pass
  (TASK-79.18's simulation mode)
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:14'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: task
ordinal: 324000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.18's fix (a fetcher reads an extent where its object's footer says) was tested without the simulation mode its brief asked for, because sim/world.rs belonged to the TASK-91.2 session then: a writer's create held in flight past a collector pass that deletes. Without it the simulation cannot reach the interleaving the bug needed. A fenced writer's create retries are also not stopped (hygiene: a writer cut off cannot know it is fenced).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote's simulation has a mode that holds a writer's create until a deleting pass has run
- [ ] #2 A simulation run in that mode fails on the code before TASK-79.18's fix and passes after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
