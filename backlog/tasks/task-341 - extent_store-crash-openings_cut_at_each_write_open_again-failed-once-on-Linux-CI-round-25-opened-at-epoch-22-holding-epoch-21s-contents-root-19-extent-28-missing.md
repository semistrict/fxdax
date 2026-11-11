---
id: TASK-341
title: >-
  extent_store::crash openings_cut_at_each_write_open_again failed once on Linux
  CI: round 25 opened at epoch 22 holding epoch 21's contents (root 19, extent
  28 missing)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 01:22'
updated_date: '2026-11-11 15:15'
labels:
  - extent_store
  - durability
dependencies: []
priority: high
type: bug
ordinal: 740000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
CI run 42935580033, test (workspace) job 130244855227 (branch t330-land at d0beb3c9d, whose extent_store is main's 7e1740cb7, 2026-11-11): 2483 of 2484 passed; this one failed at crates/extent_store/tests/crash/kept.rs:318, 'round 25: at epoch 22': the store, cut at a write and opened again, said epoch 22 but held [... (27, 7, 49673)] root 'root 19', where epoch 22's contents add (28, 5, 14245) and root 'root 21'. So an opening after a cut claimed an epoch whose commit's data it lacks: either a commit acknowledged durable that recovery lost, or the epoch counted past the contents. The same code passed in CI runs 42915833387, 42924964007 and 42932581818 and on ubu1; the test's cuts or timings vary by run (it printed no seed). Recent changes in the area: TASK-92.41 (commits spanning segments, a5dd824c5, ca1c208b7), TASK-318. Not reproduced yet; first step: find what the test's randomness is and make a failing round replayable.
<!-- SECTION:DESCRIPTION:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Again, CI run 42979578798 (t330-land rebased on main dfd9eb484), test (workspace) job 130386571778: extent_store::crash recovers_from_its_bucket_with_and_without_its_disk failed at the same kept.rs:318, 'round 25: at epoch 23', the same contents, (28, 5, 14245) missing, root 'root 23'. The rounds' contents are the same each run (the same extents, round 25 both times), so the cut or the timing varies. Two failures in two consecutive Linux CI runs of the crash tests.

Reproduced on ubu1 with main 7e1740cb7's extent_store (no TASK-330.1 change in it): cargo nextest run -p extent_store --test crash, 3 runs, one failed: recovers_from_its_bucket_with_and_without_its_disk, 'round 81: at epoch 10' (kept.rs:318). About 1 in 3 runs of the crash tests here, 3 of 4 Linux CI runs of t330-land. Main's own CI passed it in its last runs by luck.

Third and fourth Linux CI hits on t330-land (run 42979578798, its reruns): openings_cut_at_each_write_open_again, 'round 25: at epoch 23'. It now fails most Linux CI runs: every push's CI is a coin toss until it's fixed.

Main's own CI now: run 43006118431 (main 394119d9d) failed on both crash tests at once, recovers_from_its_bucket_with_and_without_its_disk 'round 25: at epoch 23' and openings_cut_at_each_write_open_again 'round 25: at epoch 22'; every other job green. Main's CI is red until this is fixed.

2026-11-11 (claude): root causes, three, all found by TASK-310's crash rounds first and fixed here apart from TASK-310's framing changes.
1. Round 25/28 (kept.rs:318, an epoch claimed without its contents): recovery kept a later commit past an earlier one not durable: a log refusing a record's runs, a segment past the base holding no record nor whole framed file, or a log not following the last commit kept, ended nothing; and an opening stopped part way removed the refusing segment first. wal::recover now ends what it keeps at each, removing the later segments first, durably (15d7a65ce; TASK-334's rules; tests logs_following_a_commit_lost_go, logs_following_the_base_are_kept, logs_past_commits_no_segment_holds_go, logs_refusing_a_record_after_whole_ones_end_what_is_kept, commits_whose_runs_a_log_refuses_go, ..._whatever_a_recovery_stopped_left, each red without its rule).
2. Round 81 (opened at epoch 4, 8 durable): a spare log, renamed from segment 3's log and then renamed segment 6's, neither rename durable, was kept by a crash under 3's name holding 6's header and records, which recovery read as 3's commits. A log's header now names its segment (LOG_MAGIC v2: salt, branch, number), and a log whose header names another holds none of its commits; store format 10 (22193d7f3; test logs_of_other_segments_hold_none_of_their_names, red without it).
3. Round 45 (kept.rs:308, Diverged): TASK-345, fixed with it.
Proof: crash tests (cargo nextest run -p extent_store --test crash, all three) 30 runs of 30 green on ubu1 under fxjob with the final code (main failed about 1 run in 3; 25 of 25 green with 1 and 2 alone, round 45 then seen once on macOS). macOS scripts/test.sh green (2239 workspace, sim 48, shuttle 37); Linux workspace on ubu1 2495 of 2497, the two failures TASK-315 (ZFS page cache, every ubu1 run) and TASK-360 (hostile timeout under load, passes alone in 14 s), neither in this code; upstream-changes --check passes; specs: check-tla.sh all 81 green.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Recovery ends what it keeps at the first commit not durable (refused runs, an empty segment past the base, a log not following), removing later segments first; logs' headers name their segments (format 10); with TASK-345's rebase fix. Crash tests 30/30 on ubu1 (main ~1 in 3 failed).
<!-- SECTION:FINAL_SUMMARY:END -->
