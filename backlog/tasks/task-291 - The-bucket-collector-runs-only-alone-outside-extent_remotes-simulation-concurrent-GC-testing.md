---
id: TASK-291
title: >-
  The bucket collector runs only alone outside extent_remote's simulation:
  concurrent-GC testing
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-09 08:37'
updated_date: '2026-11-09 08:37'
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
- [ ] #1 Every subtask is done
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
