---
id: TASK-79.34
title: >-
  The store's path is released while its instance is still alive: TASK-54's rule
  is broken in three places
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
priority: low
type: bug
ordinal: 222000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Teardown proceeds once running reaches 0, before the job drops its Arc<Connection>. holder_or_hold returns a going holder, so a resize through a dead guest changes the capacity durably, fails with EIO, and keeps the Connection alive. SyncStore and a cancelled ForkStore release the path before the close they only spawned has run. In each case the next opener or Delete gets EBUSY.

Where: crates/fxdax/src/extentd/server.rs:386, crates/fxdax/src/extentd/server.rs:217, crates/fxdax/src/extentd/server.rs:255, crates/fxdax/src/open_stores.rs:212, crates/fxdax/src/store_requests.rs:196, crates/fxdax/src/extentd/stores.rs:189, crates/fxdax/src/extentd/stores.rs:460.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-54, TASK-44.6, TASK-43.6. TASK-54 (Done) is the single race where a request's pool task notified 'drained' while still holding its Arc<Connection>. Its fix, drop(self) before notify in serve_in_turn, is in the tree. It does not cover the three paths here: the going holder from holder_or_hold on resize, SyncStore, and the cancelled ForkStore releasing the path before close. TASK-44.6 (Done) fixed a different case, 'a store opened here was released before it closed'. TASK-43.6 made SyncStore and ForkStore waits async.
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
