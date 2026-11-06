---
id: TASK-92.54
title: >-
  A store's segment count is unbounded: small fully-live segments are never
  merged, so opening reads a block of each
status: Done
assignee: []
created_date: '2026-10-23 23:09'
updated_date: '2026-11-06 06:50'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 364000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left by TASK-92.16 (accepted by the user 2026-10-23). Commits no longer scan every segment, but opening a store still reads one block of each live closed segment: 12.9 MB in 4.5 ms at 2,000 segments, 90 MB in 35 ms at 20,000 (store-bench segments). close_now has no minimum size, and cleaning takes only segments under its live fraction, so small fully-live segments are never merged and their count grows with a store's age.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store's closed segment count stays within a bound, by merging small fully-live segments or a minimum close size
- [ ] #2 store-bench segments shows opening's time flat in the store's age
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Moot after TASK-165.8 (a31f82a98): a store attached to an uploader keeps only the closed segments not yet acknowledged in the bucket, whose count the uploader's lag bound bounds (4,096 commits, 1 GiB); acknowledged ones leave the store (Segments::reap_acknowledged, crates/extent_store/src/segments.rs:320), so the count no longer grows with a store's age, and opening reads a block of each of those alone (segments.rs:186-191). A store with no uploader keeps everything, but only extent_store's own tests and fxdaxctl's offline commands run so. Small segments in the bucket are the collector's to compact.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as moot: since TASK-165.8 a host store keeps only unacknowledged segments, bounded by the uploader's lag, so their count and the open's reads no longer grow with the store's age.
<!-- SECTION:FINAL_SUMMARY:END -->
