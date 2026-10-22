---
id: TASK-88
title: >-
  extent_remote's simulation builds segments with Batch, not with extent_store,
  so the store's footer code never meets takeovers or collectors
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: medium
type: task
ordinal: 262000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left from the object-storage protocol refactor (integrate-2, review finding 'Segment footers come from two producers'): the simulation's sessions still close commits with Batch, so extent_store's footer code (Window::places, frees, forks, next_sequence at close_now) is never run against takeovers or the collector. The review traced a next_sequence/orphan case (TASK-79.3) that the simulation therefore can't reach. Each session would need an extent_store Store of its own, attached as fxdax's remote.rs start() attaches (diffing a snapshot), logic that lives in fxdax now.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The simulation's writers commit through extent_store's Store, and their segments' footers come from it
- [ ] #2 The simulation reaches TASK-79.3's case: a test seeds it and fails before TASK-79.3's fix
- [ ] #3 The extent_remote simulation tests pass, in a time comparable to now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
