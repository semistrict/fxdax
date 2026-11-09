---
id: TASK-295
title: >-
  A write-ahead log dropped by `fxdaxctl take` (Recovering::drop_wal) comes back
  after a crash, and its stale commits upload into the branch
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 09:07'
updated_date: '2026-11-09 10:42'
labels:
  - bug
dependencies: []
priority: high
ordinal: 697000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-88's simulation (seed 193 of sim::stale_creates_landing_past_deletions, profile recreated). A host's store whose log diverged was dropped (drop_wal), reopened, committed; the host crashed; recovering the store found the dropped log's segment again at the queue's head (writer 1's commit at store epoch 6, the reopened store at epoch 2). Store::closed takes nothing past a segment whose commits are not durable, so uploads stalled until the store's epoch reached 6; then the stale segment, restamped, went into the branch after the commits the new writer had, losing them and naming the dropped history. drop_wal (store.rs) removes the segments' files and calls dir.barrier(), but syncs none of the directories whose entries it removed and leaves the base file as it was: MemDir's crash keeps or drops each unsynced directory change at random, as a host's filesystem may.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A store test: drop_wal, open, commit, crash (MemDir::crash over many seeds), recover: the dropped segments are gone and the log follows the reopened store's commits; it fails before the fix
- [x] #2 drop_wal makes its removals and the empty base durable before it returns
- [x] #3 sim::stale_creates_landing_past_deletions holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fixed on branch task-88 (12733b212): drop_wal syncs segments/ and extents/ of its branch and writes the empty base before its barrier. tests/store.rs a_dropped_log_stays_dropped_through_a_crash failed at seed 2 before, passes after; sim::stale_creates_landing_past_deletions holds.

CI 42600287867 green at df22d4e59.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
drop_wal now syncs its branch's segments/ and extents/ directories and writes the empty base before its barrier, so a crash after `fxdaxctl take` cannot bring a dropped log back. Verified by tests/store.rs a_dropped_log_stays_dropped_through_a_crash (64 crash seeds; failed at seed 2 before) and sim::stale_creates_landing_past_deletions (seed 193 found it).
<!-- SECTION:FINAL_SUMMARY:END -->
