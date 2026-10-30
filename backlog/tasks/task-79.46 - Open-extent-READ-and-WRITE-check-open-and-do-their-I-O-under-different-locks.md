---
id: TASK-79.46
title: Open-extent READ and WRITE check open and do their I/O under different locks
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-30 13:08'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 234000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A READ racing a commit returns zeros as a success. A WRITE racing the first MAP of the same extent is overwritten by the VMO at commit.

Where: crates/fxdax/src/extents.rs:970, crates/fxdax/src/extents.rs:978, crates/fxdax/src/extents.rs:1085, crates/fxdax/src/extents.rs:1095, crates/fxdax/src/extents.rs:1502, crates/fxdax/src/extents.rs:1234.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-59, TASK-32, TASK-26.12. No task covers the open-extent READ/WRITE lock race. TASK-59 (To Do, cause unknown: postgres 'unexpected end of data' in a parallel index build) shows a short or wrong read and could be a symptom, but it is not this defect.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reads of open extents hold the open extents; the WRITE half was closed by TASK-121. reads_of_open_extents_hold_off_their_commit stops a read between finding its pages and reading them (a cfg(test) hook): without the fix the read returns zeros as a success; with it, passes. A race test without the hook passed unfixed, the window being nanoseconds, and was dropped.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Reads of open extents hold the open extents through the read, so a commit can't move their pages out from under it (60c868c4b's change on main at 36adb1ce4). reads_of_open_extents_hold_off_their_commit stops a read in the window: without the fix it returns zeros as a success. Passes on macOS (scripts/test.sh) and Linux (GCE r285, 1969 of 1969).
<!-- SECTION:FINAL_SUMMARY:END -->
