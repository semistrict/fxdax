---
id: TASK-79.55
title: Timestamps outside 1677-2262 are silently clamped
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
ordinal: 243000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
timespec64_to_ns clamps, but s_time_min and s_time_max are left at the TIME64 limits, so the cached time differs from the stored one until the inode is evicted.

Where: guest/linux/fs/fxfs/super.c:222, guest/linux/fs/fxfs/super.c:2360.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.8. No task covers s_time_min/s_time_max against u64-nanosecond storage.
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
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. super.c: s_time_min/max set to what 64 bits of nanoseconds hold; check timestamps_clamped NOT YET RUN.

Mutant GCE r297, without s_time_min/max: timestamps_clamped fails (13000000000 against 9223372035).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The superblock says the times Fxfs holds, so a time is clamped as it is set. timestamps_clamped; without s_time_min/max it fails 13000000000 against 9223372035 (r297). Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
