---
id: TASK-293
title: >-
  The collector never compacts a compressed segment its live extents leave
  mostly dead: its prefilter compares their logical bytes with the stored size
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-09 08:58'
updated_date: '2026-11-09 09:21'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 696000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-88's simulation, whose segments are extent_store's, compressed (seed 0 of sim::everything with compressible extents: segments/2/5, 1536 of its 3584 bytes of data live, 1232 bytes stored, left uncompacted through four passes). collector.rs compact(): `let below = |size| used < compact_below * size`, then `!below(self.objects[key].size)` skips the object before its GET: used is the live extents' logical length (Location::len), the listing's size the object's stored bytes, compressed. The prefilter is a necessary condition only while size >= data length, as Batch's uncompressed segments in the old simulation were. With compression a mostly dead compressible segment is never compacted, and its dead bytes stay in the bucket for as long as its live extents do.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The collector compacts a compressed segment whose live extents take less than compact_below of its data, without a GET of every named object each pass
- [ ] #2 A collector test with a compressed, mostly dead segment fails before the fix
- [ ] #3 The simulation's check_compacted (crates/extent_store/src/sim/world.rs, TASK-88) bounds live bytes by the data alone again, not by the smaller of the data and the stored size, and holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-88's simulation meets this at seeds past its soaks' (taken 258, everything 322): segments compress even with random extent bytes (8,007 stored of 12,288 of data), so its check_compacted mirrors the prefilter until this is fixed.
<!-- SECTION:NOTES:END -->
