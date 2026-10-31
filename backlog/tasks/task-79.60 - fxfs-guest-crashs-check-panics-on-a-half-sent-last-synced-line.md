---
id: TASK-79.60
title: fxfs-guest-crash's check panics on a half-sent last "synced" line
status: Done
assignee: []
created_date: '2026-10-21 21:00'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 248000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crash() reboots without draining the console, so a truncated line or the kernel's restart message hits the parser's unwraps, and a store that was fine fails the check.

Where: guest/tests/test_cases/src/test_fxfs_guest.rs:154, guest/tests/test_cases/src/test_fxfs_guest.rs:607, guest/tests/test_cases/src/guest_init.rs:125, guest/tests/test_cases/src/vm.rs:624.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.6.2, TASK-67. TASK-26.6.2 (Done) fixed a real durability race in the crash test, not parsing of a half-sent 'synced' line. No existing task.
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
Parsers were made tolerant in fd293cec3. Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. unit test lines_cut_short_name_no_synced_file passes on Linux (Docker).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Parsers tolerate a line cut short. lines_cut_short_name_no_synced_file; with the strict parser it panics (Docker). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
