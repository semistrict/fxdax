---
id: TASK-29.3
title: 'Object storage: compressed uploads, fetches and cache'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
updated_date: '2026-10-17 15:10'
labels: []
dependencies:
  - TASK-29.2
parent_task_id: TASK-29
priority: high
type: feature
ordinal: 97000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Once local closed segments are compressed in the shared framing (TASK-29.2), the uploader can PUT them as they are, instead of re-encoding segments from read_kept as remote.rs does now. extent_remote::fetch fetches parts with ranged GETs by part*PART offsets and checks CRCs over raw bytes; with compressed parts it must locate parts through the object table, check the stored bytes, and decompress. The local fetch cache (extent_remote::cache) can keep the compressed parts. The collector compaction (moves objects) and checkpoints must write the same framing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Uploaded segments, own extents and moves objects are compressed in the TASK-29.1 framing
- [x] #2 Fetches read a part with one ranged GET of its stored bytes, check its CRC and decompress it; the cache keeps parts compressed
- [x] #3 The extent_remote simulation, the fetch and cache tests, the fxdax remote tests and scripts/test-rustfs.sh pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uploads: segments and own extents are the store's files, PUT as they are (fxdax remote gather -> Writer::add_segment -> upload); only the start snapshot is re-encoded from read_kept. Fetches: tail first (layout::TAIL), then one ranged GET per part of its stored bytes, CRC-checked then decompressed; cache keeps stored (compressed) parts at their object offsets. reader::read and checkpoint lookups read via layout::read_tail/read_data. Collector compaction writes moves with with_footer (compressed). Tests: fetch tests (noise data keeps raw parts at part offsets) + compressed_parts_are_fetched_and_kept_so; checkpoint lookups_read_one_block reads exactly the stored parts of the block; fxdax reads_keep_what_they_fetch keeps compressed bytes; RustFS passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The uploader PUTs the store's closed segments and own extents as they are; fetches GET the tail, then each part's stored bytes, check and decompress; the cache keeps parts compressed; reader and checkpoint lookups read through the table. Verified: extent_remote simulation, fetch/cache/checkpoint/writer tests, fxdax remote tests, scripts/test-rustfs.sh.
<!-- SECTION:FINAL_SUMMARY:END -->
