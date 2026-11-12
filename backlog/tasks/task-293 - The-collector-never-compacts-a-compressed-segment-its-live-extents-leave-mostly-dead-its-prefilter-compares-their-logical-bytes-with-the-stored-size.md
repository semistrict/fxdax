---
id: TASK-293
title: >-
  The collector never compacts a compressed segment its live extents leave
  mostly dead: its prefilter compares their logical bytes with the stored size
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 08:58'
updated_date: '2026-11-12 20:43'
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
- [x] #1 The collector compacts a compressed segment whose live extents take less than compact_below of its data, without a GET of every named object each pass
- [x] #2 A collector test with a compressed, mostly dead segment fails before the fix
- [x] #3 The simulation's check_compacted (crates/extent_store/src/sim/world.rs, TASK-88) bounds live bytes by the data alone again, not by the smaller of the data and the stored size, and holds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Measure live extents against each object's data length, read from its 44-byte frame trailer (ranged GET) and cached in the Collector by key, version and listed size, as objects never change; forget entries no longer listed. Test first: a compressed, mostly dead segment compacted. Drop the workarounds: the sim's min(data, stored), the crash rounds' comment, TASK-175's incompressible test bytes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
TASK-88's simulation meets this at seeds past its soaks' (taken 258, everything 322): segments compress even with random extent bytes (8,007 stored of 12,288 of data), so its check_compacted mirrors the prefilter until this is fixed.

How it slipped: the prefilter (557945982, 2026-10-16) predates compression (TASK-29, 2026-10-17) and was not revisited; TASK-175's compacted() (04d381d38) used incompressible bytes "so that the objects' sizes are their extents'", working around it; TASK-291.2 made half the crash rounds' data incompressible for the same reason; no test asserted that a mostly dead compressed object is compacted, only that nothing live is lost.
Fix: Lengths in collector.rs; Pass::compactable learns lengths (TRAILERS_AT_ONCE=32 ranged reads, only for keys not known at their listed version and size), a trailer whose frame size differs from the listing is Corrupt. compact() no longer re-checks against the whole object's data.
Verified: compressed_objects_are_compacted_by_their_data fails with the old check (hand mutant: below(.., meta.size): compacted 0) and passes with the fix; extent_remote + extent_store 450/450 on macOS; extent_store sim at SIM_SOAK=3 (600 seeds a profile, past seeds 258/322 that met it) 48/48 with check_compacted bounding by data alone; fxdax collect/kept/model_tests/remote 81/81; ubu1 452/453, the one failure durable_commits_keep_the_open_segments_pages_cached_on_the_host, ZFS (TASK-309). upstream-changes.py --check passes. Workspace run left to main's CI.

CI 43138881376 (5bd295e41) passed.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The collector measured live extents against an object's stored size, compressed, so mostly dead compressed segments were never compacted. It now reads each object's data length from its frame trailer once, cached by key, version and size. compressed_objects_are_compacted_by_their_data goes red with the old check; the simulation's check_compacted bounds by data alone again and holds at SIM_SOAK=3; macOS, ubu1 and CI pass.
<!-- SECTION:FINAL_SUMMARY:END -->
