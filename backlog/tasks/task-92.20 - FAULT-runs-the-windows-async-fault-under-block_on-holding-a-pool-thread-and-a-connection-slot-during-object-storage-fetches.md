---
id: TASK-92.20
title: >-
  FAULT runs the window's async fault under block_on, holding a pool thread and
  a connection slot during object-storage fetches
status: In Progress
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 12:29'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 295000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
DaxWindow::fault waits for the pager without holding a thread (dax.rs:205-213), but FAULT runs it under futures::executor::block_on in a pool job (server.rs:600-610). Each fault holds a shared pool thread (pool.rs:39-45) and one of its connection's 8 WORKERS slots (server.rs:22-24) while a 1 MiB part comes from GCS (30-100 ms). A VM's faults take slots from its own device requests, and about 40 VMs booting at once fill a 128-thread pool, so every connection's requests queue.

Where: crates/fxdax/src/extentd/server.rs:22-24, crates/fxdax/src/extentd/server.rs:369-406, crates/fxdax/src/extentd/server.rs:600-610, crates/fxdax/src/extentd/pool.rs:39-45, crates/fxdax/src/dax.rs:205-213.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still present at 4432a7e. FAULT still runs window.fault/fault_page under futures::executor::block_on (crates/fxdax/src/extentd/server.rs:623-633). It holds one of the connection's WORKERS=8 slots (server.rs:25, admit 396-405) and a thread: the first request runs under block_in_place on the IO runtime (server.rs:248-251, from 93a73fc/TASK-41), and queued ones run on the shared Pool (server.rs:414; pool.rs:39-44, max (16*cpus).max(64)). DaxWindow::fault is still async and threadless (dax.rs:196-198), and FAULT is not deferred through begin_deferring/end_deferring (server.rs:1052-1056). No task covers it. TASK-79.16 is the pool and sender blocking, TASK-43.6 covers only control-API waits, TASK-91.6 is FAULT error counts. Related: TASK-79.16, TASK-43.6, TASK-91.6, TASK-41.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [ ] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
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
