---
id: TASK-92.4
title: >-
  Writer::checkpoint follows every extent's moves serially, inline in the
  uploader loop, and a reopened writer re-makes every fork's record
status: Done
assignee: []
created_date: '2026-10-22 21:06'
updated_date: '2026-11-07 13:02'
labels:
  - review-2026-10-21
dependencies:
  - TASK-165.9
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-92
priority: high
type: enhancement
ordinal: 279000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
What is left, at dde8d58d3. The review (6a0a3f8) found Writer::checkpoint probing moves/<key> once per object, one at a time, and reading every live fork's record one at a time. Since then a checkpoint lists each key prefix's moves and reads those that exist 32 at a time (MovesMaps::read, crates/extent_remote/src/moves.rs:43-74, READ_AT_ONCE; 431717757, TASK-91.2), and forks' records are made and read RECORDS_AT_ONCE (32) at a time (crates/extent_remote/src/writer.rs:49, 431-457, 488-499; 4896b828b).

Left:
- The checkpoint still follows every extent's moves in a serial loop (writer.rs:476-481; MovesMaps::follow, moves.rs:95-107): a cached lookup for most, a GET for each hop of an extent the collector moved.
- The uploader awaits it inline: every CHECKPOINT_EVERY (64) segments (crates/fxdax/src/remote.rs:77, 1101-1105), polling no uploads or waiters meanwhile, and in acknowledged() before any fork it acknowledged is answered (remote.rs:1456-1491, the checkpoint at 1485).
- A reopened writer's recorded set starts empty (writer.rs:116, 229), so record_forks re-creates every child's record, each existing one a failed create and a GET (branch::record_fork, crates/extent_remote/src/branch.rs:145-160), 32 at a time.
So with hundreds of forks of a base, a storm of forks waits on checkpoints that re-read every fork's record and re-make them after a reopen, and long runs stall in wait_lag (remote.rs:820).

TASK-165.7 (forks only in the bucket) and TASK-165.9 (the uploader in extent_store) rework this code; measure after them.

Where: crates/extent_remote/src/writer.rs:116, 229, 431-457, 463-512; crates/extent_remote/src/moves.rs:43-107; crates/extent_remote/src/branch.rs:145-160; crates/fxdax/src/remote.rs:1101-1105, 1456-1491.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Performance issues', has its evidence and a suggested fix. Related: TASK-90, TASK-39.3, TASK-91.3, TASK-79.39.
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
On main (4896b828b): Writer::record_forks and the checkpoint's reads of forks' records run RECORDS_AT_ONCE (32) at once (buffer_unordered); test forks_records_are_made_and_read_a_few_at_once (SimBucket: 65 forks, 32 requests waiting at once, acknowledging and checkpointing; serially 1). The reopened writer still re-creates every child's record (32 at a time now). Verified at 3364435c6 (the batch): macOS extent_remote and fxdax 371 passed; Linux workspace on GCE r264 1965 of 1965; guest suites on GCE r263 (release), fxfs-guest-dax, -checks-dax, -crash-dax, -checks-dax-small, -checks-dax-delay, -pgbench-dax 2 rounds each passed (pgbench-dax 1051.6 -> 863.4, 997.4 -> 889.5 tps).

2026-11-05 triage: description brought to dde8d58d3: the per-object moves probes (431717757) and the serial fork-record reads (4896b828b) are fixed; the title and what's left restated.

2026-11-06 tidy: depends on TASK-165.9 (its description says to measure after TASK-165.7, landed on main, and 165.9, which reworks the uploader and recovery this measures).

Measured after TASK-165.7 and 165.9 (2026-11-06): what was left of the three. (1) The checkpoint read every live fork child's record each time, and since 165.9 every acknowledgement carrying forks takes a checkpoint (extent_store/src/upload.rs, acknowledged: the checkpoint makes the forks), so a storm of N forks read N^2/2 records: 128 forks, 128 records a checkpoint, about 8,256 across the storm. Test checkpoints_read_a_few_forks_records_at_most (SimBucket, 128 forks): red before, 128 records read by one checkpoint against 32. Fixed in 0908fa7c6: a checkpoint reads RECORDS_AT_ONCE (32) children's records at most, in turn after the last it looked at (Inner.looked), so about 4,000 across the storm; a deleted child's fork goes within ceil(N/32) checkpoints. (2) Following moves: a cached lookup per extent after MovesMaps::read's listing, a GET only for each hop of an extent the collector moved; not a cost on a fork storm, left. (3) A reopened writer's recorded set starts empty (writer.rs:238), so its first acknowledgement re-creates every acknowledged fork's child record, a failed create and a GET each, 32 at once: once per takeover, not per checkpoint, left. The checkpoint every CHECKPOINT_EVERY (64) segments stays inline in the uploader loop; with (1) its cost no longer grows with the forks. Verified (landed as land-xk, 4bf0e45fd): macOS full suite (clippy, workspace 2086/2086, sim, Shuttle, scripts); CI 42363963416 green; GCE r894: workspace 2305/2305, sim 48/48, Shuttle 36/36, default guest suite 15/16 (extent-guest's next_sequence expectation, fixed in 101eeb943); GCE r895: extent-guest 2/2, fxfs-guest-soak 2/2. scripts/upstream-changes.py --check passes on main 0879c8272.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A fork storm's checkpoints were quadratic: each read every fork child's record (128 forks: 128 a checkpoint, ~8,256 across the storm). A checkpoint now reads 32 at most, in turn, ~4,000 across it; shown by checkpoints_read_a_few_forks_records_at_most, red before (128 against 32). The cached moves following and a reopened writer's one-off re-creation of records (32 at once) were measured as not growing per checkpoint and left. Verified by macOS and CI suites and GCE r894/r895.
<!-- SECTION:FINAL_SUMMARY:END -->
