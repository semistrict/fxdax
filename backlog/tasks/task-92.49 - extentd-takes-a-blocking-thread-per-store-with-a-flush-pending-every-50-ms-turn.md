---
id: TASK-92.49
title: >-
  extentd takes a blocking thread per store with a flush pending, every 50 ms
  turn
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-23 13:18'
updated_date: '2026-10-24 16:16'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 349000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing the thread fix (TASK-79.2, review-r1). extentd's flush_behind (crates/fxdax/src/extentd/server.rs) still takes a tokio blocking thread for every store whose flush is pending on each 50 ms turn, as main did before TASK-79.2. With hundreds of writing stores the blocking pool grows to one thread per store, each idle 10 s after. Capping flush concurrency per daemon (or max_blocking_threads on the IO runtime) would bound it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The daemon's threads stay within a fixed bound with hundreds of stores flushing at once, shown by a test like serves_hundreds_of_forks_of_a_base for writers
- [x] #2 Flushes keep their latency under that bound, measured
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

flush latency re-measured after 167cd9591 (shared helpers), median and tail against 53.7/91.9 ms before (hundreds_of_writers_flush_on_threads_they_share).

Re-measured on main after 167cd9591, GCE r66 (2026-10-24, n2-standard-8 us-central1-a), hundreds_of_writers_flush_on_threads_they_share --no-capture: 300 stores' guests committing, 46 threads kept ([14, 32, 0] io/flush/clean), at most 50 at once; a guest's 19 ordered commits durable within 55.7 ms (median) and 69.5 ms (slowest), against 53.7/91.9 ms before the shared helpers. Workspace green on Linux (r65, 2621) and macOS.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
extentd's flushes run on a bounded set of shared threads, not one blocking thread per store a turn: 300 writing stores keep at most 50 threads, commits durable in 55.7 ms median, 69.5 ms slowest (hundreds_of_writers_flush_on_threads_they_share, GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
