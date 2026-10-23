---
id: TASK-79.73
title: >-
  Simulation: hold a writer's create past a deleting collector pass
  (TASK-79.18's simulation mode)
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
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
- [x] #1 extent_remote's simulation has a mode that holds a writer's create until a deleting pass has run
- [x] #2 A simulation run in that mode fails on the code before TASK-79.18's fix and passes after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extent_remote's simulation can now hold a taken-over writer's segment creates until a collector pass deletes an object at their key. The new recreated profile uses it, and Rules::footers turns TASK-79.18's fix off. Run on the code before TASK-79.18's fix, stale_creates_landing_past_deletions fails at seed 8 with a part past its object's end. On main it holds over 200 seeds, and stale_creates_need_reads_by_footers finds a failing seed with the rule off. The tests pass on macOS and on Linux (GCE). Stopping a fenced writer's create retries, which the description calls hygiene and no AC requires, is not done.
<!-- SECTION:FINAL_SUMMARY:END -->
