---
id: TASK-241
title: >-
  A snapshot taken while its volume is written shares extents the volume then
  frees
status: To Do
assignee: []
created_date: '2026-11-08 05:38'
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
