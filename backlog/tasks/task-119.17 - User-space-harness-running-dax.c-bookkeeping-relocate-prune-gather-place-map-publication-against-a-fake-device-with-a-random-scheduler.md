---
id: TASK-119.17
title: >-
  User-space harness running dax.c bookkeeping (relocate, prune, gather/place,
  map publication) against a fake device with a random scheduler
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: low
ordinal: 455000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable/Astra: 1-2 weeks; kernel tools/testing/radix-tree shims cover xarray/bitmaps/locks; minutes-long race search instead of GCE rounds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The harness finds TASK-121 or a seeded mutant within minutes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
