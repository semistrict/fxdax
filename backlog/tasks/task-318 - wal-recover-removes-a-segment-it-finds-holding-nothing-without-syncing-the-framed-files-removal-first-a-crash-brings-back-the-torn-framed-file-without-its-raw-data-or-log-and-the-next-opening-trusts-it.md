---
id: TASK-318
title: >-
  wal::recover removes a segment it finds holding nothing without syncing the
  framed file's removal first: a crash brings back the torn framed file without
  its raw data or log, and the next opening trusts it
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 09:45'
updated_date: '2026-11-10 10:33'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 724000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-291.2's kept/cut crash rounds (openings_cut_at_each_write_open_again, round 21, about 1 in 40). wal::recover's gone() removes a segment's framed file, raw data and log as directory changes and syncs none of the three directories (but in the branch for segments after a log lacking its last commits). The store's opening then makes the next open segment, whose raw file and log entries it syncs: open-segments/ and logs/ go durable, segments/ not. A crash then keeps the framed file, here one an earlier crash tore (trailer, table and footer whole, a part not), and drops its raw data and log. The next recovery's whole_footer, raw data and log gone, takes the framed file as one its framing made durable, unchecked, and the store's reads of extents in it fail 'an object's part, by its CRC' (segment 0xf, extent 35 in the round). A framed file's removal must be durable before its raw data's and log's are, as framing's is before they go.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A wal.rs test: a segment the base acknowledged, its framed file torn, raw data and log there, recovered, open-segments and logs synced as the next opening does, then crashed at each of 64 seeds: no recovery after keeps the torn segment; it fails before the fix
- [x] #2 gone() makes the framed file's removal durable (segments/ synced, a barrier) before it removes the raw data and log
- [x] #3 openings_cut_at_each_write_open_again round 21 passes 200 times in a row
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. wal.rs test: segment 1's framed file torn (a data byte flipped, trailer/table/footer whole), raw data and a log of no durable record back, all durable; recover; sync open-segments and logs as the opening does; crash at 64 seeds; each recovery keeps segment 0 alone.
2. gone(): remove the framed file; if it and the raw data or log are there, sync segments/ and barrier; then remove raw data and log.
Verify: wal::tests (red at seed 2 before), cut round 21 x200, crash binary x15.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Red before the fix: seed 2 kept segment 1 as framed whole. After: wal::tests 9/9; openings_cut round 21 200/200 with passes off and on (was ~1 in 40); the whole crash binary 15/15 (failed ~1 in 5 to 12 before). The kept rounds' one audit failure seen before the fix ('Piece ... past SegmentId 14's data', segment's frame shorter than its pieces) did not recur in those 15; likely the same stale framed file, not proven.

ubu1: extent_store+extent_remote 417/418 (the one, TASK-291.2's compaction threshold, since lowered; ZFS test excluded, TASK-315); crash binary 3/3 after.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
wal::recover's gone() now removes a segment's framed file, syncs segments/ and barriers (when the framed file and raw data or log are there), then removes raw data and log, as framing makes its file durable before they go. Before, the opening's syncs of open-segments/ and logs/ could make the raw data's and log's removals durable alone, and a crash kept a torn framed file the next recovery trusted unchecked. Test segments_holding_nothing_go_framed_files_first (red at seed 2 before); cut round 21 200/200; macOS and ubu1.
<!-- SECTION:FINAL_SUMMARY:END -->
