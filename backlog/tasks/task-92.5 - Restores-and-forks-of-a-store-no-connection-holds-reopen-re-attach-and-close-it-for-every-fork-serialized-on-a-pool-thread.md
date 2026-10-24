---
id: TASK-92.5
title: >-
  Restores and forks of a store no connection holds reopen, re-attach and close
  it for every fork, serialized on a pool thread
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
priority: high
type: enhancement
ordinal: 280000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fork_from holds the point with Held::new, a Condvar wait on a pool thread for up to CLOSING_WAIT = 10 s (stores.rs:118, 138-141; open_stores.rs:102-123). It then opens the point, attaches it (probe, two recoveries, lease takeover, fence), forks it, and closes it with a checkpoint (stores.rs:389-418); control-API fork does the same (425-451). One open plus attach measured 1.16-1.21 s, and a restore is about 18 sequential requests. In a zygote fan-out, the k-th restore waits about (k-1)×T, restores past about 10-13 fail with EBUSY, each waiter pins a shared pool thread, and each release wakes every key's waiters (open_stores.rs:193-196).

Where: crates/fxdax/src/extentd/stores.rs:118, crates/fxdax/src/extentd/stores.rs:138-141, crates/fxdax/src/extentd/stores.rs:389-418, crates/fxdax/src/extentd/stores.rs:425-451, crates/fxdax/src/open_stores.rs:102-123, crates/fxdax/src/open_stores.rs:193-196, crates/fxdax/src/extentd/server.rs:671, crates/fxdax/src/extentd/server.rs:713.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Still there. Restore's fork_from still holds the point with Held::new, a Condvar wait for up to CLOSING_WAIT = 10 s on the calling thread (crates/fxdax/src/extentd/stores.rs:118, 137-142, 408-418; open_stores.rs:101-122). Unless the point is open for followers, it opens and attaches it with take_store and drops it after each fork. The control-API fork now waits with hold_async (TASK-43.6) but still opens a Here store per fork and closes it (stores.rs:425-450). Nothing keeps a daemon-opened point open. Paths::release still wakes every waiter of every key (open_stores.rs:190-196). Related: TASK-43.6, TASK-43.8, TASK-39.1, TASK-75, TASK-79.34.
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
