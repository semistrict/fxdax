---
id: TASK-79.16
title: >-
  extentd blocks shared threads on one VM process's socket, and reads that
  process's requests with no backpressure
status: In Progress
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-24 12:29'
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
