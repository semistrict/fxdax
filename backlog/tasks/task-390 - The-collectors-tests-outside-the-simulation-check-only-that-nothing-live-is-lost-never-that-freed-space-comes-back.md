---
id: TASK-390
title: >-
  The collector's tests outside the simulation check only that nothing live is
  lost, never that freed space comes back
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 20:44'
updated_date: '2026-11-12 20:44'
labels:
  - testing
dependencies: []
priority: medium
ordinal: 767000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-293 (compressed segments never compacted) stayed hidden because collector tests check safety only. extent_store's simulation has the liveness oracle (check_garbage, check_compacted in sim/world.rs, which found it); the TASK-291 tests (crash rounds' kept passes, fxdax's storage kept and model tests) and collector unit tests do not. Make the simulation's checks one shared oracle in extent_remote::testing, its live branches from their records, and run it at the end of those tests.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extent_remote::testing has one oracle of what passes run alone left that they should have collected: garbage, and objects left uncompacted, measured against their data
- [ ] #2 The simulation's end checks are that oracle, and hold at SIM_SOAK=3
- [ ] #3 The crash rounds' kept test and fxdax's collector model test end on it
- [ ] #4 Each goes red with a collector that never compacts, or never deletes, put back by hand
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
