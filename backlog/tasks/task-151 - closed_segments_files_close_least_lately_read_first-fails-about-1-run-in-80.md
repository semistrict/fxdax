---
id: TASK-151
title: closed_segments_files_close_least_lately_read_first fails about 1 run in 80
status: Done
assignee: []
created_date: '2026-11-01 19:11'
updated_date: '2026-11-08 04:42'
labels: []
dependencies: []
priority: low
type: bug
ordinal: 540000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_store test, 30 failures in 2400 at the base commit, 27 in 2400 with fix/host-audit, so it predates that work: 17 files opened where 8 were expected. Cause not found.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The cause is found and fixed, or the test states what it can hold
- [x] #2 It passes 2400 runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Stress the test at main and at the commit it was filed at, macOS and Linux, alone and with its whole binary under load. 2. Fix if it reproduces; close with the evidence if not.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08 closure sweep: not reproduced anywhere. closed_segments_files_close_least_lately_read_first passed 2,400 of 2,400 each: at main 2a5cd577f on macOS and in Linux Docker (aarch64); at c0255b389 (main when it was filed, 2026-11-01 18:56) on macOS and in Linux Docker; at 67740ed72 (before the lineage's removal, ecdf8da16) on macOS; at main with eight_segments' settled() removed, macOS. And the whole store binary, 300 iterations on macOS (~35,000 test runs, all its tests side by side): all passed. Since 2b823d65a (TASK-165.9) eight_segments waits for every framing to end (settled) before reading, which rules out the one source of extra file opens behind the test's back, a framing on its own thread reading a segment's files. No CI failure of it since 2026-11-05 (22 failed CI runs' logs searched: none name it; green main CI 42442410406 passed it).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed, not reproducible: 2,400 of 2,400 runs pass at main and at the commit it was filed at, on macOS and on Linux, plus 300 runs of the whole store binary under its own load. The test has waited for framings to settle since 2b823d65a (TASK-165.9), the one background reader of segment files. No code changed. Reopen with the run's seed and environment if it fails again.
<!-- SECTION:FINAL_SUMMARY:END -->
