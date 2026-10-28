---
id: TASK-119.12
title: >-
  fxfs-guest-checks-dax-small: the checks with a 16-32 MiB window, regions full,
  own-space writable maps, reclaim under load
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 450000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable: tests use a large window; the sandbox team runs it full. window_regions_full -> map_own writable in own space, MAP_MANY own pieces, -ESTALE drops (TASK-79.106) and reclaim are untested where TASK-101 lived.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The case runs in the default set and reaches regions_full and reclaims (counters)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
