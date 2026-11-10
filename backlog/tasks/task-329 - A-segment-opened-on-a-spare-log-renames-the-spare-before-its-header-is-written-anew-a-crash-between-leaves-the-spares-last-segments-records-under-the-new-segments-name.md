---
id: TASK-329
title: >-
  A segment opened on a spare log renames the spare before its header is written
  anew: a crash between leaves the spare's last segment's records under the new
  segment's name
status: Done
assignee: []
created_date: '2026-11-10 13:48'
updated_date: '2026-11-10 15:25'
labels:
  - extent_store
  - crash
  - TASK-301
dependencies: []
priority: high
type: bug
ordinal: 729000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-310 (crash round 270 of extent_store's crash tests, recovers_a_prefix_of_its_commits, failing once large extents that don't compress are framed in place, which shifts the round's operations). segments::create_open reused a spare log (TASK-301) by renaming spare-<n> to logs/<segment>, then writing and syncing its new header. A failure or crash between the rename and the header's sync left logs/<segment> with the spare's old header, whose salt validates the old segment's records: the opening took them for the new segment's commits, Synced runs past its empty raw file (the audit: a piece past segment 6's data). Fixed with TASK-310's commits: the spare's header is written anew and synced at its spare path before it is renamed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The crash rounds pass with the header written before the rename; red with the rename first (round 270)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
segments::create_open writes and syncs a spare log's new header at its spare path before renaming it the segment's log (90b4d8943). Verified: extent_store's crash rounds pass; red with the rename first (round 270 of recovers_a_prefix_of_its_commits, pieces past segment 6's data); macOS and ubu1 suites green.
<!-- SECTION:FINAL_SUMMARY:END -->
