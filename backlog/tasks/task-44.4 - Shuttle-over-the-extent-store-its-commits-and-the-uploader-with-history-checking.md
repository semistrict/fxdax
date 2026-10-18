---
id: TASK-44.4
title: >-
  Shuttle over the extent store, its commits and the uploader, with history
  checking
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
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
- [ ] #1 Writers (write, commit, free, fork), the closer and the uploader run under Shuttle over mem.rs and the simulated bucket
- [ ] #2 A history checker requires the bucket's branch, after any schedule, to equal the store as of some durable commit, never older than one acknowledged durable; and no read of an extent freed and released
- [ ] #3 Mutation checks: releasing kept frees early, closing a segment with writers in it, or acknowledging before upload each makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
