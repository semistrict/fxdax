---
id: TASK-287
title: fxdax lib test warns dropping_copy_types in store_table.rs (drop of a usize)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:01'
updated_date: '2026-11-10 09:30'
labels:
  - bug
dependencies: []
priority: low
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
scripts/test.sh on macOS (TASK-257's run, main 142195ae2): warning: calls to std::mem::drop with a value that implements Copy does nothing, crates/fxdax/src/store_table.rs: hold.hold().retire(|| drop(store.cuts.fetch_add(1, Ordering::SeqCst))). Use let _ = or a block.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 No dropping_copy_types warning in the workspace's test build
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. retire(|| { store.cuts.fetch_add(..); }).
2. Blind spot: the code is in store_table's shuttle_tests, compiled only by the shuttle part, where warnings don't fail; and the clippy part doesn't deny rustc warnings. test.sh's clippy part: -D warnings, and a clippy of the Shuttle builds (extent_store, physmem, fxdax, follower, follower_sim --features shuttle --tests).
3. Red with the drop back, green without.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
How found: a warning in scripts/test.sh's output on macOS. Blind spot: the drop is in store_table's shuttle_tests, compiled only by the shuttle part, whose warnings fail nothing; the clippy part denied clippy's disallowed_methods alone, so rustc's warnings passed everywhere. Class test: the clippy part now passes -D warnings and clippies the Shuttle builds too (extent_store, physmem, fxdax, follower, follower_sim --features shuttle --tests). Red with the drop put back (error: calls to std::mem::drop with a value that implements Copy), green without; the clippy part clean on macOS and ubu1. Look-around: no other warning in the workspace's test builds or the Shuttle builds. guest/tests is not clippy-clean (dozens of clippy lints, pre-existing), so its build was not put under -D warnings. Structure: fine; the fix is the gate.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
retire's closure no longer drops a usize; scripts/test.sh's clippy part fails on rustc warnings and covers the Shuttle builds, where this one hid. Verified red/green on macOS; clippy part clean on macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
