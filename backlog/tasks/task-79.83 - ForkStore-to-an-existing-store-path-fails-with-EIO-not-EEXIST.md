---
id: TASK-79.83
title: 'ForkStore to an existing store path fails with EIO, not EEXIST'
status: In Progress
assignee: []
created_date: '2026-10-23 09:16'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 342000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2c (TASK-79.78). A ForkStore retried to an existing `to` gets EIO (INTERNAL): fork_host's create_dir io::Error carries no Errno, and extentd/stores.rs's errno_of maps it to EIO, while CreateStore and ImportImage check path.exists() and give EEXIST.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 ForkStore to an existing path fails with EEXIST, as CreateStore and ImportImage do, pinned by a test
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

On main (2026-10-24): round 2b rebased onto main and pushed with the guest batch, 08790a954..b158ce82b; Firecracker 535defc6e..6610d88f1 on fxdax-v1.16.1. Green on macOS (scripts/test.sh, 2508) and Linux (GCE r65: workspace 2620, Firecracker extent tests, 17 guest cases). The criterion of a test failing before the fix isn't recorded here: check the round 2b commit that names this task before ticking it.
<!-- SECTION:NOTES:END -->
