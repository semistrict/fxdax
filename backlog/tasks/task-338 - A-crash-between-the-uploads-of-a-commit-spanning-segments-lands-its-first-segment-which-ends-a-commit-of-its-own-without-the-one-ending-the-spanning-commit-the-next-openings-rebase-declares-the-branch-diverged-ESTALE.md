---
id: TASK-338
title: >-
  A crash between the uploads of a commit spanning segments lands its first
  segment, which ends a commit of its own, without the one ending the spanning
  commit: the next opening's rebase declares the branch diverged (ESTALE)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 23:47'
updated_date: '2026-11-11 22:52'
labels:
  - durability
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 738000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-310's bucket crash rounds (crash::recovers_from_its_bucket_with_and_without_its_disk, round 45, 'the store doesn't open: another writer's commits moved the branch on from the write-ahead log's', about one run in ten on TASK-310's branch). A segment that filled holding commits (TASK-92.41) ends a commit of its own (27) and goes on with the next (28), which the following segments end. The uploader took and uploaded segments concurrently (upload::InFlight, FuturesOrdered), and the first could land without the others; a crash then left the bucket's end at commit 27 inside that segment, the next commit unended. extent_remote::Wal::rebase finds the branch's last commit as a head of the log only where the segment before it doesn't go on with a commit (its 'between' rule), so it found none and returned Diverged, the store refusing to open. Present on main since TASK-92.41 (extent_remote is unchanged by TASK-310; a deterministic test of rebase with that bucket state fails there): the rounds hit it by timing.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The uploader never lands a segment that goes on with a commit without the segments after it to the one ending it: they are taken together and the first lands last; a crash leaving the later ones landed alone leaves them void past the gap, which rebase stamps anew
- [ ] #2 Queue::take's test covers the grouping; the bucket crash rounds pass repeatedly
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10 (claude, with TASK-310): fixed in extent_store: wal::Queue::take takes a spanning commit's segments only all together, to the one ending it (queued, framed, its commit durable), past the uploader's limit if they come first; upload::InFlight::upload lands the group's first segment only once the rest have (one future in the first's slot, try_join_all of the rest then the first; the rest's slots done after it). A crash may then leave the later ones landed past the gap the first leaves, void at the fence, which rebase stamps anew (extent_remote's logs_landed_past_a_gap_are_stamped_again), never the first alone. Blind spot: the bucket crash rounds reach it only by timing (one run in ten on TASK-310's branch, none seen in ~15 runs of main's): extent_remote's rebase tests had a first segment holding no commit of its own (logs_of_commits_spanning_segments_land_whole), whose landing alone rebase handles. A deterministic test of rebase with the bad bucket state fails on main and here (kept out of the tree: the state is now unreachable; rebase still refuses it, by design of 'between'). Tests: wal::tests::queues_give_commits_spanning_segments_whole; crash rounds 25 runs of the whole binary, none failing (were 2-3 in 25).

Also (bucket crash rounds, round 65, traced): acknowledging a spanning commit's segments one at a time let a continuation segment's file leave for the host's cache (it held no live data) before the base moved past the commit; a crash before the next acknowledgement left the commit's first segment and its last locally with a hole between, which recovery read as the commit unended, losing it and the next, durable. wal::Queue::acknowledge now holds a segment going on with a commit and returns the group once the one ending it is acknowledged, so Store::acknowledged lets them go together, the base then past the commit (queues_give_commits_spanning_segments_whole checks the hold).

2026-11-11 (claude): AC #1's uploader grouping dropped with TASK-345, whose Wal::rebase follows a segment ending a commit and going on with the next landed alone (round 45, the case this was for); the acknowledgement hold (round 65) stays, lands with TASK-310 (queues_hold_acknowledged_segments_going_on_with_a_commit). Crash tests 10/10 on ubu1 with TASK-310.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Diverged on a lone landed segment: fixed by TASK-345's rebase instead of uploader ordering; a spanning commit's segments let go of together once its last is acknowledged (round 65's hole).
<!-- SECTION:FINAL_SUMMARY:END -->
