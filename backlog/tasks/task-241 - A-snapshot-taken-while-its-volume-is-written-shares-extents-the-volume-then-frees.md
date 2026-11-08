---
id: TASK-241
title: >-
  A snapshot taken while its volume is written shares extents the volume then
  frees
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-08 05:38'
updated_date: '2026-11-08 06:05'
labels:
  - bug
dependencies: []
priority: high
ordinal: 676000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.40, the model test snapshotting its volume: `RootVolume::snapshot_volume` (crates/fxfs/src/object_store/volume/snapshot.rs) flushes the source, then commits a transaction that makes the snapshot store from the source's flushed layers and freezes the source's data owner (`Allocator::snapshot`). A transaction of the source that commits between the flush and that commit is in the source's mutable layer, not in the snapshot's layers, yet what it does to the allocator is done: an extent it frees, the snapshot still references, as nothing froze it yet. Seen with a file closed whose tombstone the graveyard, batching as a guest's does, ran during the snapshot's flush: remounted, the snapshot's graveyard tombstoned the file again, its extent was no longer allocated, and the journal failed with an inconsistency ("isn't allocated"), the filesystem unusable. A write landing there would leave the snapshot reading an extent the source freed. fxdax's `Image::snapshot_volume` allows writes to the source while it snapshots ("may be in the snapshot or not"), so a snapshot of a mounted, busy volume can do this. The model test lets the graveyard settle before each snapshot until this is fixed; `model_tests::a_snapshot_of_a_file_closed_but_not_yet_tombstoned` reproduces it, ignored, naming this task.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A transaction of the source committing at any point while a snapshot is taken is in the snapshot whole or not at all, and nothing the snapshot references is freed by it
- [ ] #2 model_tests::a_snapshot_of_a_file_closed_but_not_yet_tombstoned passes, no longer ignored, and the model test snapshots without letting the graveyard settle first
- [ ] #3 A test of crates/fxfs commits writes and frees of the source concurrently with snapshot_volume and checks the snapshot with fsck and by reading it back
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design: a snapshot is atomic with its source's commits.
1. A commit gate per store: `FxFilesystem::commit_transaction` takes a read lock on a new `LockKey::Commits { store_object_id }` for each store whose mutations the transaction holds (not BeginFlush/EndFlush, which a flush adds), after its write locks and before the journal commit, held until it commits. `snapshot_volume` takes it for writing before the flush lock, and holds it until its transaction commits: no commit of the source lands between the flush, whose layers the snapshot takes, and the snapshot's commit, which freezes the source's data owner. Taken first, it waits only for commits in flight, which may compact the journal and flush the source, the flush lock not yet held: no deadlock; the flush's own commits touch the source only by BeginFlush/EndFlush.
2. Frozen at apply: a transaction built before the freeze and committed after it (waiting at the gate) may hold a Deallocate, or a Reference, of an extent now frozen, which `deallocate` would have left out had it been built after. The allocator's apply leaves those of frozen owners' extents out too, live and in replay alike (the Snapshot mutation precedes them in the journal).
3. Not fixed: an allocation built before the freeze and committed after it is recorded under the frozen owner, which the source alone references: never freed one by one, a leak until the owner goes (the snapshot isn't harmed).
Tests first: model_tests::a_snapshot_of_a_file_closed_but_not_yet_tombstoned un-ignored (red); a crates/fxfs test churning writes and frees of the source concurrently with snapshot_volume, fsck and reading both back (red if it reaches the window); then Op::Snapshot without the graveyard settling, the model test's hunts.
<!-- SECTION:PLAN:END -->
