---
id: TASK-79.59
title: >-
  Multi-VM host halves block forever in accept() for a guest that died before
  connecting
status: In Progress
assignee: []
created_date: '2026-10-21 21:00'
updated_date: '2026-10-24 12:29'
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
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Handed over (2026-10-24): fixed on branch review/r2b-extentd (48 commits on main b5eb2fc8, rebased onto 709a7017), not on main. Verified on Linux (GCE): workspace 2614/2614 and every Shuttle suite; guest fxfs-guest, -dax, -checks, extent-guest, -follow, -live-fork, -daemon-lost, -snapshot, -snapshot-dax, -pgbench, -pgbench-dax OK. fxfs-guest-checks-dax failed on TASK-79.104's guest crash (fixed since on main), and fxfs-guest-follow-torture failed 1 of 3 on main alone and 1 of 3 with these commits (TASK-79.58's flake). Blocked only on a push: the branch's vendor/firecracker points at 6610d88f1, three commits on semistrict/firecracker fxdax-v1.16.1 past 1fbc852f9 (535defc6e TASK-79.41, 8ea641a6d and 6610d88f1 TASK-92.13), in /Users/ramon/src/fxdax/.claude/worktrees/r2bm/vendor/firecracker; push those first, then the branch to main, rebasing onto main (guest-only changes since).
<!-- SECTION:NOTES:END -->
