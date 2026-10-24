---
id: TASK-79.16
title: >-
  extentd blocks shared threads on one VM process's socket, and reads that
  process's requests with no backpressure
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
priority: medium
type: bug
ordinal: 204000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Replies are sent on the pool thread that served the request, holding the Sender mutex across poll(-1). A VMM that stops reading exhausts the shared pool, and going()'s peer_gone takes the same mutex inside the global Paths lock, blocking every open and release daemon-wide. The receive loop also never stops reading: the waiting queue is unbounded and messages can be 64 MiB, so a streaming VMM OOMs the daemon.

Where: crates/fxdax/src/extentd/wire.rs:221, crates/fxdax/src/extentd/wire.rs:239, crates/fxdax/src/extentd/wire.rs:387, crates/fxdax/src/extentd/server.rs:199, crates/fxdax/src/extentd/server.rs:275, crates/fxdax/src/extentd/server.rs:382, crates/fxdax/src/extentd/pool.rs:39.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-34, TASK-41, TASK-43.6, TASK-56. No task covers replies sent under the Sender mutex with poll(-1), peer_gone under the Paths lock, or unbounded receive queues. TASK-34 built the shared pool and the sender that waits for room. TASK-41 notes moved serving onto the I/O thread. TASK-43.6 is about control-API waits.
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

Fix: e6cf07d9b, 41cfd874d, 4b7727be0, bcf88f5e0 (tests: vm_processes_that_stop_reading_hold_up_no_other, vm_processes_that_leave_replies_unread_are_let_go, tenants_connections_are_bounded, reads_past_a_messages_payload_are_refused). Red before for e6cf07d9b and 4b7727be0; 41cfd874d's accounting race has no red test (its message says so). Verified on main at 043d413da (2026-10-24): macOS scripts/test.sh (2508 workspace tests and the Shuttle and scripts suites), Linux on GCE r65 (workspace 2620, Firecracker extent tests, 17 guest cases), scripts/upstream-changes.py --check. Failing-first evidence: the fix commit's message records the test failing on its parent on GCE (round 2b audit, 2026-10-24).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd no longer blocks shared threads on one VM process's socket, and bounds what it reads of each process's requests.
<!-- SECTION:FINAL_SUMMARY:END -->
