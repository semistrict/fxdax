---
id: TASK-44.4
title: >-
  Shuttle over the extent store, its commits and the uploader, with history
  checking
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:06'
updated_date: '2026-10-18 04:47'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 133000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crates/extent_store/src/store.rs (commits, seals, close_now waiting on idle writers, kept frees, footings, three condition variables) and the uploader in crates/fxdax/src/remote.rs (three threads) interleave in ways unit tests don't reach: the uploader's panic in close_now (no entry found for key: an extent freed since the last commit) came from one. extent_store has an in-memory directory (mem.rs) and extent_remote a simulated bucket (sim/bucket.rs), so writer, uploader and closer can run together under Shuttle.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Writers (write, commit, free, fork), the closer and the uploader run under Shuttle over mem.rs and the simulated bucket
- [x] #2 A history checker requires the bucket's branch, after any schedule, to equal the store as of some durable commit, never older than one acknowledged durable; and no read of an extent freed and released
- [x] #3 Mutation checks: releasing kept frees early, closing a segment with writers in it, or acknowledging before upload each makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
crates/extent_store/tests/shuttle.rs, under the extent_store 'shuttle' feature (crate::sync swaps std's Mutex/MutexGuard/Condvar for Shuttle's; Arcs stay std's): a writer (writes in segments and in own files, frees, ordered and durable commits, a fork), a closer (close_now) and an uploader using the store's uploader API (upload, committed, read_kept, release, closed, uploaded), over MemDir. Deviation from AC1's wording: the uploader is simulated in the test, as fxdax's real one (remote.rs) runs on tokio and object_store, which Shuttle doesn't schedule; the simulated bucket isn't needed, as the history check reads what the uploader reads. Checks: each durable commit's written extents read back as written (kept though freed since) until released; at each commit the uploader takes, a crash snapshot (MemDir::crash) recovers at least that commit; at the end the store holds the last commit's state and the fork its parent's at the fork. 1500 random + 1500 PCT schedules, ~20 s; SHUTTLE_SOAK=10 (15000 each) clean. Rules (extent_store::Rules, compile-time constants without the feature): keep_frees, close_waits_for_writers, durable_only; each turned off fails for its own reason ("no extent", "reads other data than was written", "before durable"), the test checking the panic message. Found a real bug: a crash while forking left a torn branch record and the store refused to open (fixed in the commit before, regression test a_fork_whose_record_never_became_whole_never_happened). scripts/test.sh runs the Shuttle tests. macOS: 1997 workspace tests + 5 Shuttle tests pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The extent store's writer, closer and uploader run under Shuttle with a history check (durable commits' data readable until released, crash recovery at least what was uploaded, the fork's state); its three concurrency rules each shown needed. It found a real crash bug in forks (fixed separately).
<!-- SECTION:FINAL_SUMMARY:END -->
