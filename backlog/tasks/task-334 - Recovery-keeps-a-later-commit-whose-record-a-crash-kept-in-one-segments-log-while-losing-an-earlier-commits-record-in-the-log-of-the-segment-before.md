---
id: TASK-334
title: >-
  Recovery keeps a later commit whose record a crash kept in one segment's log
  while losing an earlier commit's record in the log of the segment before
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 22:11'
updated_date: '2026-11-11 15:15'
labels:
  - durability
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 737000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-310's Shuttle crash rounds (crash::crashes_recover_prefixes_of_their_commits_concurrently, round 20, once the framings' syncs moved): one flush appends its commits' records to the logs of their segments at once (Store::append_records), and a crash before they are synced may keep any of their blocks: here commit 26's record in segment 11's log, while commit 25's record in segment 10's log was lost. wal::recover took segment 10 (no framed file, a log holding no record) as holding nothing durable and removed it, then closed segment 11 again from its log with commit 26: the branch recovered at epoch 26 without commit 25's changes (extent 16 lost), not a prefix of its commits. Recovery checked that a segment closed again is followed by the next commit (forward), never that it follows the last commit kept (backward), and a segment holding nothing was skipped with no check. Present on main (the recovery code is the same); the crash rounds' schedules had not reached it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A segment closed again whose log's first commit is past the one after the last commit kept goes, with those after it, durably; a deterministic test of recovery (two logs, the later one's record kept) is red without it
- [x] #2 The Shuttle and serial crash rounds pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10 (claude, with TASK-310): fixed in wal::recover: a segment closed again whose log's first commit is past the one after the last commit kept (the last kept segment's, or the base's, whichever is later) goes, with those after it, durably. Blind spot: recovery checked continuity forward only (a segment closed again is followed by the next commit), and skipped a segment holding nothing without any check; the serial crash rounds keep each flush's appends in order, and the Shuttle rounds' schedules never kept a later log's record while losing an earlier one's until TASK-310 moved the framings' syncs. Tests (red without the fix): wal::tests::logs_following_a_commit_lost_go (two logs, the later record kept) and wal::tests::logs_following_the_base_are_kept (the base counts as kept: the first version of the check took only the last segment kept, and crash::recovers_from_its_bucket_with_and_without_its_disk lost durable commits after an acknowledged leftover, rounds 44/50/53/69). Looked around: recovery's other order assumptions (whole framed files trusted in any order) are TASK-310's: trusted now only up to the first segment not whole.

Also (same rounds, rarer): a commit spanning two segments, its record in the second's log whole and its runs there durable, its record in the first's checking runs the first's raw data didn't hold (a crash before the flush's data sync, at once with its records, was durable): log_records now says whether it refused a record, and such a segment, and those after it, go (wal::tests::commits_whose_runs_a_log_refuses_go, red without it). Also: a leftover the base acknowledged, its raw file back without its log, no longer breaks the run of trusted framed files (segments_acknowledged_brought_back_break_no_whole_run; crash::openings_cut_at_each_write_open_again hit it).

Also, the opening-cut rounds (crash::openings_cut_at_each_write_open_again, round 28, traced): (a) recovery's removal of the segments a refused log ends removed that segment first; an opening stopped part way left its raw file without its log, which the next took as gone durable, and kept the later segment's commit 3 without commits 1-2's extents. Now the later segments go first, durably, then it (cut). (b) a segment with its raw file or log there, past those the base acknowledged, holding no record and no whole framed file, held no durable commit (the flush that would have made one durable syncs its entries), so it ends what is kept too: the crash may lose its log's entry or its record while keeping a later segment's record of a commit spanning both. Test: wal::tests::commits_whose_runs_a_log_refuses_go_whatever_a_recovery_stopped_left (recovery stopped after each operation, 8 crash seeds each), red without either rule.

2026-11-11 (claude): AC #1's rules landed on main with TASK-341 (15d7a65ce), ahead of TASK-310; the trust in whole framed files only up to the first that isn't, and the leftover that breaks no whole run, come with TASK-310's framing change, which needs them. Crash rounds: 30/30 on ubu1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Recovery ends what it keeps at a log not following the last commit kept (or the base), a log refusing a record's runs, or an empty segment past the base, removing later segments first, durably; landed with TASK-341.
<!-- SECTION:FINAL_SUMMARY:END -->
