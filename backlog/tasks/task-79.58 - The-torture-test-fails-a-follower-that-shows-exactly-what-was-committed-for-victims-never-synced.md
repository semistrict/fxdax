---
id: TASK-79.58
title: >-
  The torture test fails a follower that shows exactly what was committed, for
  victims never synced
status: In Progress
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
ordinal: 246000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Compaction commits between syncs, so followers can see an unsynced victim with its name but no data, and the test reports STALE. It is a concrete case of TASK-42.10 AC2's model error.

Where: guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:738, guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:997, guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:1003, crates/fxfs/src/object_store/journal.rs:2321, guest/rust/fxfs-linux/src/device.rs:1028, guest/linux/fs/fxfs/super.c:1847.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.10, TASK-42.7. TASK-42.10 (In Progress) is about the shaped file showing a page's pre-write state. Its mechanism was found and a fix (the fill barrier) built. Its AC2 names model errors only generally. No task tracks unsynced victims showing up as STALE.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The local check of a held victim takes EMPTY too; the history's check_held judges which is right. No deterministic reproduction (a compaction's commit between create and write); r291 runs fxfs-guest-follow-torture.

GCE r292: fxfs-guest-follow-torture passed; r293 3 rounds.

The local check takes EMPTY, follower_history judges it; fxfs-guest-follow-torture passes r292 and r293 3 rounds. AC1 open: no deterministic reproduction.
<!-- SECTION:NOTES:END -->
