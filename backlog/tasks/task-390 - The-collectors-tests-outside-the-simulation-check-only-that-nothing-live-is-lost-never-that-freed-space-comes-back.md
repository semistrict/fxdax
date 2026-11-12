---
id: TASK-390
title: >-
  The collector's tests outside the simulation check only that nothing live is
  lost, never that freed space comes back
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 20:44'
updated_date: '2026-11-12 22:51'
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
- [x] #1 extent_remote::testing has one oracle of what passes run alone left that they should have collected: garbage, and objects left uncompacted, measured against their data
- [x] #2 The simulation's end checks are that oracle, and hold at SIM_SOAK=3
- [x] #3 The crash rounds' kept test and fxdax's collector model test end on it
- [x] #4 Each goes red with a collector that never compacts, or never deletes, put back by hand
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Oracle: extent_remote::testing::uncollected (the simulation's check_garbage and check_compacted, merged, live branches and origins from records). Outside the simulation it met writers that never checkpointed after compaction: their states name segments compacted and deleted, readers follow the moves, so the forward way is needed for every branch, not only points (the sim's end checkpoints hid that). Crash kept rounds: 8 passes alone till quiet, then the oracle, every round; fxdax model end: Collecting::collected_alone. Hand mutants: never compacting fails both (left uncompacted ...), never deleting fails both (garbage left ...). macOS: sim holds tests SIM_SOAK=3 22/22 (rule-off searches slow on main too: TASK-391; seed 4985's violation unchanged), extent_remote+extent_store 426/426, fxdax collector 82/82; ubu1 268/268.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One liveness oracle, extent_remote::testing::uncollected, now ends the simulation, the crash rounds' kept test and fxdax's collector model test; red with a collector that never compacts or never deletes.
<!-- SECTION:FINAL_SUMMARY:END -->
