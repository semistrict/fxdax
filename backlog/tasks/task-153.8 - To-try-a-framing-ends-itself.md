---
id: TASK-153.8
title: 'To try: a framing ends itself'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:53'
updated_date: '2026-11-03 13:39'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 552000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Framing end is driven from outside, across closings, Pending.closings, framings and Phase::Framing: TASK-41.1, 79.69, 79.99, 92.16, 150.2. Flushes would only wait for it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Decided, with a prototype
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (2026-11-03). A segment's framing (its data and footer framed into its framed file off the commit that closed it) is ended today by whoever comes next: the commit's flush (Pending.closings), a flush with no commit (end_closings, TASK-150.2), close_now inline, the branch's closing (State.closings, durability.waiting(), TASK-79.99), a fork (State.framings). Each of TASK-41.1, 79.69, 79.99, 92.16, 150.2 was a path that forgot one.

Decided: a framing ends itself. One path for every closing, whatever closed it (a commit, a write for room, close_now, a fork's commit):
1. Refactor first: the store's state and its idle condvar shared (Arc) with what runs off the store's threads.
2. Store::frame_behind: counts the framing in Segments, runs it on a thread of its own, or on the caller's (a directory whose I/O is its callers', a host refusing threads) with the state held. The framing frames, syncs its file and the directories, then ends itself holding the state: the segment Closed, read from its framed file, its raw file to go once the commits its footer names are durable (State.framed_raw, which a flush making them durable, or the branch's closing, removes); or the store failed. It notifies.
3. Everything else only waits: a flush for the segment its commit closed (that commit's data is in its framed file), close and fork for none framing, close_now for its own (off the locks, as now).
4. Gone: State.closings, Pending.closings (a segment number), State.framings, end_closings, framing_ended, the close() special case for queued commits' closings, rules flushes_end_closings and flushes_wait_for_every_framing (79.99's class is impossible: close waits for the one count). New rule framings_end_themselves (off: a closing for room starts framing only at the next commit, 150.2's shape): needs_flushes_to_end_closings becomes needs_framings_to_end_themselves; needs_flushes_to_wait_for_every_framing turns close_waits_for_framings off on its scenario.
5. Proof: extent_store's tests and Shuttle suites; each needs_ test red with its rule off.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented (1477b7222), as planned, with one change found by its test: a flush waiting for every framing begun before it held a guest's commit behind a close_now framing (requests_served_during_a_framing_by_pct deadlocked), what TASK-41.1 had removed. So a commit's flush waits only for the segment its commit closed, and the uploader's take of closed segments (Store::closed) waits for those being framed whose commits are durable: what TASK-150.2 needed, a closing for room with no commit after, the uploader now finds framed whatever comes next.

Structure: one record of running framings (Segments.framing, by segment), each framing removing itself as it ends (Segments::framed / framing_failed, Phase::Failed for one that failed, which failed the store); raw files of framed segments go once their commits are durable, at the next flush or the branch's closing (Segments::raw). The state and its condvar are Arc'd so a framing thread ends itself under the state lock; a framing run on the caller's thread (a serial directory, a host refusing threads) ends with the state the caller holds (spawn_or_return gives the work back).

Tests: needs_framings_to_end_themselves (rule framings_end_themselves off: a closing for room framed by the next commit, 150.2's shape) red as expected; needs_closes_to_wait_for_framings_after_a_failed_flush (79.99's scenario, close_waits_for_framings off) red as expected; new uploads_of_what_a_closing_for_room_closed_by_pct runs the 150.2 scenario under PCT with every rule, which had no all-rules run before, and found the flush-waiting mistake above. extent_store 219/219, Shuttle 61/61 and 7/7 (macOS).

Verified: macOS scripts/test.sh at e8e186485: workspace 2051/2051, Shuttle 61, 7, 6, 21, 10, 18, scripts 135 OK; the extent_remote simulation 37/37. GCE fxdax-test-t1537-r1 (us-east4-a, n2-standard-4): Linux workspace 2215/2215 and the same Shuttle suites; its guest stage stopped at a test_cases host test (XFS loop umount busy, no fxdax code), so r2 ran the guest cases alone: fxfs-guest-dax, -live-fork, -migrate-dax, -migrate-tenant-dax, -snapshot, -snapshot-dax, -snapshot-volumes, -snapshot-volumes-dax, -suspend-dax all OK, test_cases 53/53. Both hosts deleted. scripts/upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Decided and done (1477b7222): a segment's framing ends itself, whatever closed it; a commit's flush, the uploader's take, close_now, forks and the branch's closing only wait. Removes State.closings, Pending.closings, end_closings and two rules whose classes (TASK-150.2, 79.99) are now impossible. needs_framings_to_end_themselves and needs_closes_to_wait_for_framings_after_a_failed_flush red with their rules off; a new all-rules PCT run of 150.2's scenario found and fixed a flush waiting too broadly. Verified macOS and GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
