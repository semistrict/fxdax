---
id: TASK-79.41
title: Resizes are neither serialized nor matched to their answers
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-24 15:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 229000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Concurrent resize_held calls can leave the store smaller than the guest was told. In the VMM, Resizing::answered unconditionally clears a newer shrink and adopts a stale answer's capacity.

Where: crates/fxdax/src/store_requests.rs:137, crates/fxdax/src/extentd/server.rs:735, vendor/firecracker/src/vmm/src/devices/virtio/extent/mod.rs:257, vendor/firecracker/src/vmm/src/devices/virtio/extent/mod.rs:273, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:640.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.6, TASK-37, TASK-37.2, TASK-43.6. TASK-44.6 (Done) built store_requests.rs resize_held under Shuttle but did not serialize concurrent resizes. No task covers the VMM's Resizing::answered taking stale answers.
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

Fix: d37a6a561, Firecracker 535defc6e (Shuttle resizes_at_once_agree_*, needs_resizes_one_at_a_time, shrinks_unanswered_tell_the_guest_the_capacity_kept; FC answers_to_shrinks_no_longer_asked_change_nothing); all failed before per d37a6a561. Verified on main at 043d413da (2026-10-24): macOS scripts/test.sh (2508 workspace tests and the Shuttle and scripts suites), Linux on GCE r65 (workspace 2620, Firecracker extent tests, 17 guest cases), scripts/upstream-changes.py --check. Failing-first evidence: the fix commit's message records the test failing on its parent on GCE (round 2b audit, 2026-10-24).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Resizes are serialized and matched to their answers.
<!-- SECTION:FINAL_SUMMARY:END -->
