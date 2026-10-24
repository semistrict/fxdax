---
id: TASK-79.59
title: >-
  Multi-VM host halves block forever in accept() for a guest that died before
  connecting
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-24 16:46'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 247000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest panic resets the VM and Firecracker exits 0, while the host waits in a blocking accept() until the runner's TIMEOUT, and the panic message is only in the console file.

Where: guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:178, guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:258, guest/tests/test_cases/src/test_fxfs_guest_follow.rs:124, guest/tests/test_cases/src/test_fxfs_guest_live_fork.rs:77, guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs:206, guest/tests/guest-agent/src/main.rs:26.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.7, TASK-39.3, TASK-39.1. No task covers the host halves' blocking accept() when the guest dies before connecting.
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
Handed over (2026-10-24): fixed on branch review/r2b-extentd (48 commits on main b5eb2fc8, rebased onto 709a7017), not on main. Verified on Linux (GCE): workspace 2614/2614 and every Shuttle suite; guest fxfs-guest, -dax, -checks, extent-guest, -follow, -live-fork, -daemon-lost, -snapshot, -snapshot-dax, -pgbench, -pgbench-dax OK. fxfs-guest-checks-dax failed on TASK-79.104's guest crash (fixed since on main), and fxfs-guest-follow-torture failed 1 of 3 on main alone and 1 of 3 with these commits (TASK-79.58's flake). Blocked only on a push: the branch's vendor/firecracker points at 6610d88f1, three commits on semistrict/firecracker fxdax-v1.16.1 past 1fbc852f9 (535defc6e TASK-79.41, 8ea641a6d and 6610d88f1 TASK-92.13), in /Users/ramon/src/fxdax/.claude/worktrees/r2bm/vendor/firecracker; push those first, then the branch to main, rebasing onto main (guest-only changes since).

On main (2026-10-24): round 2b rebased onto main and pushed with the guest batch, 08790a954..b158ce82b; Firecracker 535defc6e..6610d88f1 on fxdax-v1.16.1. Green on macOS (scripts/test.sh, 2508) and Linux (GCE r65: workspace 2620, Firecracker extent tests, 17 guest cases). The criterion of a test failing before the fix isn't recorded here: check the round 2b commit that names this task before ticking it.

Round 2b audit (2026-10-24): on main and green (Verified on main at 043d413da (2026-10-24): macOS scripts/test.sh (2508 workspace tests and the Shuttle and scripts suites), Linux on GCE r65 (workspace 2620, Firecracker extent tests, 17 guest cases), scripts/upstream-changes.py --check. Failing-first evidence: the fix commit's message records the test failing on its parent on GCE (round 2b audit, 2026-10-24). minus the failing-first part). Left:

a reproduction on 0345d1274^ (a multi-VM host half whose guest dies before connecting, hanging in accept until TIMEOUT); the helper's test cannot fail on the old code.

Red/green on GCE r68 (2026-10-24, us-east1-b), fxfs-guest-follow with every guest dying before it connects (FXDAX_GUEST_CMDLINE=init=/bin/false: 'Kernel panic - not syncing: Requested init /bin/false failed'): on 0345d1274^ the host half blocked in accept until the runner's 'TIMEOUT (120s)'; on main it failed at once, the writer's console with its panic in the test's log.txt. Workspace green on Linux (r65, 2621) and macOS.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Multi-VM host halves wait for a guest's connection no longer than its VM runs (Firecracker::accept, 0345d1274): a guest dead before connecting fails the test at once with its console, where it hung until the runner's timeout; shown red/green on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
