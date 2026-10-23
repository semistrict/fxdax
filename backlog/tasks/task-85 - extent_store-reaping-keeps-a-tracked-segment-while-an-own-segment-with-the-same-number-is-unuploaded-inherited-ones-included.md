---
id: TASK-85
title: >-
  extent_store: reaping keeps a tracked segment while an own segment with the
  same number is unuploaded, inherited ones included
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-22 13:26'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
priority: low
type: bug
ordinal: 259000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while restructuring the segments (integrate-2, Segments): the reaping filter checks `unuploaded.contains(&number.number)` by number alone, so an inherited segment whose number matches an own segment not yet uploaded is kept too. It leaks the inherited segment's claim until the own one is uploaded.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test with a fork whose own segment shares an inherited one's number shows the inherited one reaped
- [x] #2 Fixed by matching segment IDs; the extent_store and extent_remote suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A commit's reaping kept any segment whose number the uploader had yet to upload, so an inherited segment numbered like one of the branch's own unuploaded segments was kept too, along with the branch's claim on it. Reaping now keeps only the branch's own segments for the uploader, matching branch and number (212aadd3). inherited_segments_go_though_own_ones_numbered_so_are_kept forks a branch whose own segment 0 waits for its uploader while it lets the inherited segment 0 go. The fork's footprint was 3 segments before the fix and is 2 after. The extent_store and extent_remote suites pass on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
