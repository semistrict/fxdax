---
id: TASK-79.84
title: An OPEN with `from` opens a leftover fork handle instead of forking
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-23 09:16'
updated_date: '2026-10-24 15:21'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 343000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2c (TASK-79.78). Since TASK-79.78 a failed fork whose deletion stops partway leaves the fork's handle, which DeleteStore finishes deleting. An OPEN with `from` (extentd/server.rs, the open path) finds that handle and opens it instead of forking, so the guest gets the stale half-made fork. In TASK-42's area of extentd.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An OPEN with `from` whose target is a leftover handle of a failed fork forks anew or fails, never opens the leftover
- [x] #2 A test covers it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): fixed on branch review/r2b-extentd (48 commits on main b5eb2fc8, rebased onto 709a7017), not on main. Verified on Linux (GCE): workspace 2614/2614 and every Shuttle suite; guest fxfs-guest, -dax, -checks, extent-guest, -follow, -live-fork, -daemon-lost, -snapshot, -snapshot-dax, -pgbench, -pgbench-dax OK. fxfs-guest-checks-dax failed on TASK-79.104's guest crash (fixed since on main), and fxfs-guest-follow-torture failed 1 of 3 on main alone and 1 of 3 with these commits (TASK-79.58's flake). Blocked only on a push: the branch's vendor/firecracker points at 6610d88f1, three commits on semistrict/firecracker fxdax-v1.16.1 past 1fbc852f9 (535defc6e TASK-79.41, 8ea641a6d and 6610d88f1 TASK-92.13), in /Users/ramon/src/fxdax/.claude/worktrees/r2bm/vendor/firecracker; push those first, then the branch to main, rebasing onto main (guest-only changes since).

On main (2026-10-24): round 2b rebased onto main and pushed with the guest batch, 08790a954..b158ce82b; Firecracker 535defc6e..6610d88f1 on fxdax-v1.16.1. Green on macOS (scripts/test.sh, 2508) and Linux (GCE r65: workspace 2620, Firecracker extent tests, 17 guest cases). The criterion of a test failing before the fix isn't recorded here: check the round 2b commit that names this task before ticking it.

Fix: aa409d9b3 (tests: restores_fork_anew_over_forks_that_failed, forks_marked_failed_are_found_so); the restored VM opened the failed fork's branch before. Verified on main at 043d413da (2026-10-24): macOS scripts/test.sh (2508 workspace tests and the Shuttle and scripts suites), Linux on GCE r65 (workspace 2620, Firecracker extent tests, 17 guest cases), scripts/upstream-changes.py --check. Failing-first evidence: the fix commit's message records the test failing on its parent on GCE (round 2b audit, 2026-10-24).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
An OPEN with from forks anew over a leftover failed fork's handle.
<!-- SECTION:FINAL_SUMMARY:END -->
