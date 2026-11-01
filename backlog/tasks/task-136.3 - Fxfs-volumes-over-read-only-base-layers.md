---
id: TASK-136.3
title: 'Fxfs: volumes over read-only base layers'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-30 09:08'
updated_date: '2026-11-01 02:09'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-10 - Shared-volumes-Fxfs-volumes-over-read-only-bases.md
parent_task_id: TASK-136
priority: medium
type: feature
ordinal: 470000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A volume whose bottom LSM layers belong to a base is the overlay itself: merging by key and tombstones give its semantics, with no whiteouts and no file-level copy-up. It also keeps mounts cheap, because the sandbox's allocator and root store never learn of the base.

Today `StoreInfo` lists layers as root-store object IDs, a forced major compaction merges every immutable layer, and each layer counts towards metadata reservations at 1.5x its size. See doc-10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Writes, renames and deletes in the volume are private to it, and reads see them over the base
- [x] #2 No compaction, forced major compactions included, rewrites or releases a base layer, and tombstones over base records stay
- [x] #3 Base layers count towards no metadata reservation, and base extents are never freed, reflinks of them included
- [x] #4 fsck passes on a filesystem holding such a volume, and it survives crash and journal replay
- [x] #5 A volume made from a base volume's StoreInfo and layer extents reads as the base did, without replaying the base's journal: made in one transaction whatever the base's size, and opening it costs what opening any volume of that size does
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Format 61: StoreInfoV61 lists base_layers (each a layer's size and extents) and base_branches; StoreInfoV52 migrates.
2. base_layer.rs: BaseLayerObject, a LayerObject reading a base layer through the device, validating what a manifest says.
3. ObjectStore: open base layers below the store's own; compaction keeps them (tree::flush, follower path); they count towards no reservation and no version upgrade.
4. Allocator: extents of base branches are never freed, always shared and not referenced; an import refuses a branch the allocator holds. In memory, from StoreInfo, so no allocator format change.
5. RootVolume::publish_volume writes a manifest (a StoreInfo with no layers of its own); new_volume_over_base imports it in one constant-sized transaction.
6. fsck accepts base extents. FakeDevice gets reads falling through to an attached device.
7. Tests in fxfs: reads, private writes, compaction, reservations, reflinks, snapshots, deletion, crash and replay, fsck, lazy opens, refusals.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented: StoreInfoV61 (format 61), base_layer.rs, compaction and reservations leave base layers out, the allocator keeps base branches in memory from StoreInfo, so no allocator or mutation format change. publish_volume writes the manifest, a StoreInfo with no layers of its own; new_volume_over_base takes one transaction. Opening a volume over a base read the fork 3 times with 3 files and 4 with 503: a layer's two ends and its bloom filter, which grows with the layer. Layers read this way skip Fxfs's per-block checksums, which stay in the publisher's extent records. Files preallocated in overwrite mode in a base can't be overwritten, as with snapshots. take_flushed_layers keeps base layers but no test covers it yet: followers open all volumes, so it runs when a writer flushes one.

take_flushed_layers is tested now. Validation on the final tree: scripts/test.sh passes on macOS (1876 workspace tests) and Linux (2015), upstream-changes --check passes, check-guest passes, guest kernel suite on arm64 as before this work. AC #1 is left unchecked: making the volume is one transaction whatever the base's size, but opening it reads the layer's bloom filter, which grows with the layer (3 reads with 3 files, 4 with 503). Ramon decides whether that meets it.

Ramon accepted the bloom filter read on 2026-10-31 as close enough to constant time, so criterion 1 is reworded to say what holds, and is now #5. test_base_reads_as_it_was shows the reads as it opens: 3 with 3 files, 4 with 503. Not measured on a Linux-sized checkout: TASK-136.1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fxfs volumes sit over read-only base layers: format 61 lists them in StoreInfo, compaction, reservations and version upgrades leave them out, the allocator never frees base extents, and new_volume_over_base makes a volume in one transaction from a manifest. Verified by the test_base_* tests in volume.rs (reads, private writes, compaction, reservations, reflinks, snapshots, crash and replay, fsck, refusals), and in real guests by fxfs-guest-shared-volumes(-dax). scripts/test.sh passes on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
