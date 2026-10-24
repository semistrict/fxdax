---
id: TASK-92.13
title: >-
  FREE_MANY runs on the queue's taker thread, one blocking daemon call per
  extent
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-22 21:06'
updated_date: '2026-10-24 15:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 288000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
quick() counts every FREE as quick without checking FREE_MANY (worker.rs:129-144), so a FREE_MANY popped alone is served on the taker (302-305). It frees each extent with its own call_blocking (547-560), and meanwhile the queue takes in nothing. The frame run served about 50k FREEs at 205-218 µs each, about 2.4 extents each, which blocked queue intake for about 10 s in total, and the next flush waits on them (journal.rs:1833). A 257-extent batch takes about 8.7 ms serially.

Where: vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:129-144, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:302-305, vendor/firecracker/src/vmm/src/devices/virtio/extent/worker.rs:547-560, crates/fxdax/src/extentd/client.rs:339-342, crates/fxfs/src/object_store/journal.rs:1833.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. On main at 4432a7e: Partly fixed. Firecracker 0a6f1e332 (fxdax 93a73fc, TASK-41) removed quick() and the taker's inline serving. Each request is now a future that runs on the worker only until its first await, so a FREE_MANY no longer blocks queue intake (vendor/firecracker/.../extent/worker.rs:100-128). Still there: FREE_MANY awaits free_async once per extent in sequence (worker.rs:522-536), each a separate FREE round trip (crates/fxdax/src/extentd/client.rs:395-398). There is no concurrent join and no daemon-side FREE_MANY, so a 257-extent batch still takes 257 serial round trips. Related: TASK-41, TASK-79.22.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A measurement or a test shows the cost before the change (requests, copies, lock hold or time on the workload named)
- [x] #2 The change removes or bounds it, shown by the same measurement, and the macOS and Linux (GCE) suites pass
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

Fix: c6588b97d, 2235d1945, Firecracker 8ea641a6d, 6610d88f1 (tests: FC test_frees_many_in_one_request, extents_freed_together_are_freed_in_one_request); 257 requests in 21.97 ms before, 1 in 2.52 ms after. Verified on main at 043d413da (2026-10-24): macOS scripts/test.sh (2508 workspace tests and the Shuttle and scripts suites), Linux on GCE r65 (workspace 2620, Firecracker extent tests, 17 guest cases), scripts/upstream-changes.py --check. Failing-first evidence: the fix commit's message records the test failing on its parent on GCE (round 2b audit, 2026-10-24).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FREE_MANY is one daemon request, not one blocking call per extent on the taker thread.
<!-- SECTION:FINAL_SUMMARY:END -->
