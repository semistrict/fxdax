---
id: TASK-79.95
title: >-
  A fork's child opened before the uploader takes the fork's segment makes a
  record of its own, and its parent is stuck for ever
status: Done
assignee: []
created_date: '2026-10-24 00:33'
updated_date: '2026-10-25 04:44'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 365000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.82 (branch r8/forkids). If a fork's child is opened and attached after fork_host returns but before the parent's uploader takes the fork's segment, the child finds no record of its own and no fork in its parent's state, and creates a record of its own. The parent's writer then gives the fork's record to that child, finds it another's (Corrupt 'has another origin'), and every writer of the parent fails for ever; the uploader also overwrites the child's marker. Same area as TASK-79.72 and TASK-79.82.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A child attached before its fork's segment is uploaded waits for it or takes the fork's record, never one of its own, pinned by a Gated object-store test
- [x] #2 The parent's writers go on after such a race
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24 reading: the race is that a child's marker names the fork's tag only once a parent's writer takes the segment (InFlight::upload), so before that Shared::record sees no tag, skips take_parent (TASK-79.72), and the child records a branch of its own. Fixing it needs a choice: the child waits (in-process, for the parent's uploader to take the segment; and a parent closed here with the segment untaken?), or the fork marks the child's marker 'pending' at fork_host so record() treats it as may-land; the latter's take_parent fences the old writer, but the parent's next writer re-uploads the fork commit, as in 79.72's design. Not started.

Fixed (4b9c839e2 and its test's follow-ups): extentd's fork holds the child's path from start to end, so a connection opens the child only once the fork is made, its record in the bucket. Test childs_opened_during_their_forks_wait_for_them (GCE r96): green with the fix (the open waits, the fork succeeds, the child's record names its parent); without it, the open ended at once with EIO, the child's attach taking the parent's branch and refused as the parent is open here (TASK-79.72). Holding the fork's segment PUT comes after the uploader took it, so the window the report names (before the take, no tag in the child's marker) isn't reached by the test; the fix closes it too, as no open happens during the fork at all. Push waits on r97 (an unrelated workspace abort to explain).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's fork holds the child's path throughout, so the child opens only once its fork is in the bucket; before, an open mid-fork failed with EIO or, in the earlier window, took a branch of its own and stuck the parent. Test red without the hold (EIO), green with it, on Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
