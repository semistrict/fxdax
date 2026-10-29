---
id: TASK-119.19
title: >-
  Host race suite: ExtentStore's operations raced in pairs, what commits write
  checked
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 23:39'
updated_date: '2026-10-28 23:39'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 459000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-121's last loss was a host atomicity race: ExtentStore::write checked an extent wasn't open, released the open lock, then wrote the store, while a writable MAP opened the extent from the store between; its commit wrote zeros over the write. Guest checks found it only after hours of GCE runs and diagnostics; a host test racing the two failed in 0.03 s. Race detectors can't see it: every access was locked. The host's operations (write, map writable and read-only, commit, free) raced against each other directly, many times, with what the commit writes checked, find this class fast.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each pair of ExtentStore operations that can touch one extent concurrently (write, writable map, read-only map, commit) has a test racing them from a barrier many times and checking what the next commit wrote
- [ ] #2 Every test fails with the TASK-121 bug put back (the open lock released before the store write), and passes on main
- [ ] #3 The suite runs within the fxdax crate's tests in under 30 s
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A helper in extents.rs tests: race(rounds, prepare, steps, check): per round a fresh extent, prepare run alone, then each step on its own thread released by one barrier, then commit and check.
2. Pairs with one outcome whichever wins: write x writable map (opening), write x read-only map (extent already written, open in the store), write x commit, two writes at other offsets x writable map, write x writable map x commit's predecessor ordering via commit after.
3. Fold the existing writes_racing_the_mapping_that_opens_their_extent_are_committed into the helper.
4. Verify on GCE: each fails with the fix reverted (hand mutant), all pass on main, timing under 30 s.
<!-- SECTION:PLAN:END -->
