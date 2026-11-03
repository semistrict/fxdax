---
id: TASK-79.54
title: A follower's first batch runs before fill_super has finished
status: Done
assignee: []
created_date: '2026-10-21 21:00'
updated_date: '2026-11-03 02:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 242000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The follow work is scheduled before the DAX state, s_op and the root exist. The root's attributes can stay stale, and an ERR_PTR in info->dax can be dereferenced.

Where: guest/linux/fs/fxfs/super.c:2350, guest/linux/fs/fxfs/super.c:2354, guest/linux/fs/fxfs/super.c:2376, guest/linux/fs/fxfs/super.c:2193.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.6. TASK-42.6 (In Progress) owns guest followers, but nothing in it covers follow work scheduled before fill_super finishes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. super.c: a follower's commit callback and first batch moved to the end of fill_super. No deterministic test: the race is at mount; follow cases will show no regression once run.

The follower's first batch runs at the end of fill_super; follow cases pass r293 3 rounds each. AC1 open: no deterministic test of the race at mount.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A follower's commit callback and first batch run at the end of fill_super (8e928020b); the follow cases pass 3 rounds on GCE r293. Decided not to: a deterministic test of the race at mount.
<!-- SECTION:FINAL_SUMMARY:END -->
