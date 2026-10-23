---
id: TASK-92.46
title: >-
  Fxfs's allocator merges all its layers on every journal flush: compaction cost
  grows with the square of the files
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 06:48'
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
- [ ] #1 The allocator compacts in tiers, merging only newer layers, keeping tombstones and marked_for_deletion until a full merge
- [ ] #2 import-bench's allocator bytes compacted grow about n log n from 20k to 100k files, measured before and after
- [ ] #3 fsck and the crash tests pass, with a test that crashes during a partial allocator compaction
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
