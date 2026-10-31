---
id: TASK-79.103
title: >-
  A control ForkStore of a snapshot point extentd keeps open fails on a fence
  from another host
status: Done
assignee: []
created_date: '2026-10-24 03:39'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 374000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b (TASK-92.5). extentd now keeps a snapshot's point open for its restores; restores were made to cope with a point whose branch another host's lease fenced, but stores::fork (control ForkStore of a kept point) still fails on such a fence.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A ForkStore of a kept point another host's lease fenced reopens it and forks, or fails with an errno saying so, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. stores::fork of branch 0 retries a kept point's failed fork with another new branch: the failed fork's ID stays in the bucket, so a caller-named branch gets one try. extentd test, mutant killed; fxdax 371 of 372 in Docker Linux, the one left needing sudo.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A kept point's failed fork retries with a new ID, a caller-named one failing once. control_forks_of_a_point_fork_it_anew_once_another_writer_took_its_branch, mutant killed. Linux workspace 1986/1986 and the Shuttle suites on GCE r292, macOS scripts/test.sh 1852 and the rest, scripts/upstream-changes.py --check.
<!-- SECTION:FINAL_SUMMARY:END -->
