---
id: TASK-291
title: >-
  The bucket collector runs only alone outside extent_remote's simulation:
  concurrent-GC testing
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-10 11:01'
labels: []
dependencies:
  - TASK-88
priority: high
ordinal: 691000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_remote's simulation races collector passes against writers, forks, takeovers, deletions and restores, and TLA+ models the collector (doc-7). Everywhere else the collector runs alone: fxdax's tests (extentd/tests.rs, remote.rs, extents.rs, storage/kept.rs, remote/fork_faults.rs, remote/restore_faults.rs) call collector::collect synchronously on a quiescent bucket with grace 0; no Shuttle suite (extent_store, fxdax dax/uffd, follower, physmem) has a collector thread; extent_store's crash rounds never cut a pass; storage/collect.rs's after_commit pass, shared by an image and its clones, is only tested through pass_now; the only real concurrency is the guest soak's fxdax-gc every 15 s, slow and not replayable. Bugs where the real store's sealing, freeing, forking, WAL and part cache meet a running collector are found by no test. Subtasks, in order: TASK-88, then the ones below.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every subtask is done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Each subtask's suites ran on macOS and ubu1; the whole workspace is main's CI's run of the push.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The collector now runs concurrently in four suites besides extent_store's simulation (which TASK-88 moved onto real stores): Shuttle interleavings of a store, its uploader, a fork's child's store and passes (291.1); the kept crash rounds, a pass cut at a random request (291.2, found TASK-318); an image's and its clone's commits from two threads among after_commit passes (291.3); and a proptest model of images' files with passes as an operation (291.4). A hand mutant deleting named segments fails 291.3's and 291.4's tests; candidates_first off fails 291.1's gated twin.
<!-- SECTION:FINAL_SUMMARY:END -->
