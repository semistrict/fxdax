---
id: TASK-119.9
title: >-
  Pause points: named steps in dax.c a check holds and releases, scripting exact
  interleavings (evacuation gather/MAP/place, seal begin, map_write vs gate,
  map_chunk extents vs gate)
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 447000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable and Astra (2026-10-28): fxfs_dax_delay sleeps <=200 us a quarter of the time while a MAP takes ~400 us, so races are luck, and a seed cannot replay thread scheduling. A pause at a named step, released by the check, makes an interleaving exact and repeatable; counters prove the overlap happened.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A check pauses an evacuation after its gather, writes/truncates/fsyncs, releases it, and verifies contents
- [ ] #2 Each pause point has a check that reaches it (counter)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
