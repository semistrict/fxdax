---
id: TASK-92.46
title: >-
  Fxfs's allocator merges all its layers on every journal flush: compaction cost
  grows with the square of the files
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 331000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found measuring TASK-92.44 (round 2a). Allocator's Flusher::finish (crates/fxfs/src/object_store/allocator.rs, as upstream) merges every immutable layer into one on each journal flush (`info.layers = vec![new]`). Over the extent store each allocation is an extent of its own, so records never coalesce and there is about one per file, and every flush rewrites them all. Counted with a probe during import-bench (20k -> 100k small files): allocator 74 compactions, 16.4 MB, 98 ms -> 395 compactions, 401 MB, 2,593 ms (x24.5 bytes for x5 files); object stores x7, the n log n of size-tiered merging. At 100k files the allocator's compaction is about 11% of the import's time and grows quadratically; the same holds for any long-lived store. It is tied to fxdax's dying-extent records (AllocatorInfo.dying, extent_table), deleted owners (marked_for_deletion) and snapshots.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The allocator compacts in tiers, merging only newer layers, keeping tombstones and marked_for_deletion until a full merge
- [x] #2 import-bench's allocator bytes compacted grow about n log n from 20k to 100k files, measured before and after
- [x] #3 fsck and the crash tests pass, with a test that crashes during a partial allocator compaction
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The allocator's compactions now pick their layers as an object store's do, with tree::layers_to_merge split out of tree::flush, and merge only the newer layers. A partial merge writes removals and the records of owners marked for deletion as they are. All layers are merged only for a major compaction, once a marked owner's extents are freed, or while a layer is of an older version (c9321568, with follow-ups b0625de5, 2e80bff3, 613b2903 and doc fixes). Every change to the ported code carries an fxdax: marker. From 20,000 to 100,000 files, import-bench's allocator bytes compacted went from x24 (16.9 MB to 404 MB) to x6.0 (16.8 MB to about 101 MB, 0.61-0.65 s), where n log n is x5.8. The new allocator tests failed on the old code and pass on macOS and GCE Linux. They include test_crash_writing/ending/purging_compaction_of_newer_layers, which crash a partial compaction, then remount and fsck. Fxfs's crash and fsck tests and the guest e2e checks pass as well.
<!-- SECTION:FINAL_SUMMARY:END -->
