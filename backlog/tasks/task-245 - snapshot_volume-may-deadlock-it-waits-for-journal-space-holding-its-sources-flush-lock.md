---
id: TASK-245
title: >-
  snapshot_volume may deadlock: it waits for journal space holding its source's
  flush lock
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 06:26'
updated_date: '2026-11-08 06:46'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 679000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Noted with TASK-241's fix. `RootVolume::snapshot_volume` (crates/fxfs/src/object_store/volume/snapshot.rs) takes `LockKey::flush(source)`, flushes the source, then makes its transaction with `Options::default()`, whose `reservation_for_transaction` waits in `Journal::check_journal_space` while the journal is past its reclaim size. What frees it is a compaction, which flushes the stores with journaled mutations, the source among them if anything committed to it since the snapshot's flush, and that flush waits for the lock the snapshot holds: neither goes on. Before TASK-241's commit gate, any commit to the source between the two did that; with it, the gate holds the source's commits back, but whether a compaction still flushes the source, or the root store's own flush needs anything the snapshot holds, is unchecked. Production snapshots a mounted volume (fxdax's `Image::snapshot_volume`), whose journal may be near full.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test shows whether snapshot_volume can hang waiting for journal space while it holds the flush lock (a seeded interleaving or Shuttle), before and after TASK-241's gate
- [ ] #2 If it can, it no longer does: the test passes, and a snapshot of a busy volume with a small journal completes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Wait for journal room before taking any lock; the snapshot's transaction with skip_journal_checks, as a store's flush's. Test: a compaction held up on another volume's flush lock, the journal full, the snapshot waiting for room, the lock let go.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: real. test_snapshot_volume_waits_for_the_journal_holding_nothing hangs on main and on land-h17 (TASK-241's gate): the snapshot waited for room holding the source's flush lock (the journal's waiting hook saw snapshot_volume on its stack), the compaction, let go of the other volume's lock, waited for the source's flush lock; both hung (the test's 10 s). Fixed on land-h18 a0e9e2c26 (stacked on land-h17), CI 42456071252: the test passes; fxfs, fxfs-posix, follower, follower_sim and fxdax's image tests 922/922 on macOS. Seeded hammering alone (64 rounds of writes racing snapshots on a 16 KiB journal) never reached it: compaction kept up.
<!-- SECTION:NOTES:END -->
