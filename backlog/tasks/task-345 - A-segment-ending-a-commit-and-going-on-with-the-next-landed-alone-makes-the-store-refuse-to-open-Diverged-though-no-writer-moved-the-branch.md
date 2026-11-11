---
id: TASK-345
title: >-
  A segment ending a commit and going on with the next, landed alone, makes the
  store refuse to open (Diverged) though no writer moved the branch
status: Done
assignee:
  - '@claude'
created_date: '2026-11-11 07:59'
updated_date: '2026-11-11 15:15'
labels:
  - extentd
  - tla
dependencies: []
priority: high
type: bug
ordinal: 740000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket (TASK-149.45) modelling commits spanning segments (TASK-92.41) with a segment that ends one commit and goes on with the next (Segments::record: "the first alone may hold commits, those made before it filled"; specs/bucket's Can "spanOn").

TLC trace (1 host, 1 branch; BucketSpans.cfg with Hosts {h1}, Segs 4, MaxMints 1, Can {commit, mint, span, spanOn, crash}):
1. h1 opens the root: fence at segment 0.
2. Guest mints x; commit 1 (plain) in segment 1.
3. Commit 2 spans: segment 1, not uploaded, filled after commit 1, goes on with it (Contents { commits: [1], continued: Some(Continued { commit: 2, extents: [x] }) }); segment 2 ends it (commits: [2]).
4. Segment 1 uploads (commit 2 durable locally) and lands; segment 2 does not (in flight).
5. h1 crashes keeping its disk (or the uploader is displaced and takes the branch again: Store::rebase, live).
6. h1 opens again: recovery applies 0 and 1 (last_commit 1, begun commit 2), fences at 2 (begun dropped).
7. Wal::rebase: last = fenced.last_commit = 1; heads = [base 0, 1, 2]; position 1 has head 1 but between(1) is false (segment 1 goes on with commit 2); position 0's head is 0: no position, so Err(Diverged), though the branch's last commit is the WAL's own and no other writer committed. The store refuses to open (ESTALE) until fxdaxctl take drops the WAL: commit 2, durable locally (the guest's fsync returned), is lost.

Taking segment 1 as landed loses commit 2's run x (the fence left it unended); restamping segment 1 whole applies commit 1 twice. A fix restamps the landed segment's continuation alone after the fence (its footer's continued, its data as it was; the commits it ended, their extents, frees, root and forks dropped), then the rest; or keeps a segment holding commits from going on with the next.

Blind spot: extent_store's simulation (sim/world.rs, fn refused) takes every Diverged as another writer's commits past the log and drops the WAL, never checking that the branch moved on (its last commit neither the log's nor its base); extent_remote's wal tests have no segment ending one commit and going on with the next.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Wal::rebase puts a WAL after a branch whose last commit ends in a landed segment going on with the WAL's next commit: no Diverged, the next commit kept whole and applied once
- [x] #2 An extent_remote wal test of that segment landed alone, red on main
- [x] #3 extent_store's simulation fails a Diverged whose branch's last commit is the log's own or its base
- [x] #4 specs/bucket: BucketSpans's Can includes spanOn and holds, its Opened modelling the fix
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11 (TASK-149.45's agent): the fix sketched above, modelled in a scratch copy of specs/bucket (Opened: a landed prefix may end at a segment whose last commit is the branch's and that goes on with the next; that segment restamped after the fence with its continuation alone, commit/ext/own/free/fork dropped, then the rest), holds over BucketSpans.cfg with "spanOn" added to Can: 5,675,698 distinct states, 2 min, no violation; and over the 1-host trace's model. specs/bucket keeps "spanOn" in no model until this lands (its header says so); add it to BucketSpans's Can with the fix.

2026-11-11 (claude): Wal::rebase finds, when no place between commits has the branch's last commit, the segment ending it and going on with the next: it takes the segments before it as landed and keeps it, stamped again after the fence with its continuation alone (commits, extents, own, frees, root, capacity, forks dropped; next_sequence kept), then the rest; Rebased { landed, ended } gives its footer as it landed. Rules::wal_continues. extent_store: Store::open leaves the ended segment's own files for the cache (the bucket's state holds its commits' extents); Store::rebase, live, writes the base past its commits first, then State::landed_ending places those extents where they landed (else they'd be read in the restamped object, whose data the branch holds no more of but the continuation's). Tests: extent_remote wal::tests::logs_whose_segment_ending_a_commit_and_going_on_landed_alone_go_on (red with the rule off: Diverged); extent_store tests/store.rs logs_whose_segment_ending_a_commit_and_going_on_landed_alone_go_on (live rebase; red without landed_ending: commit 1's extent left in the segment). Simulation: World::refused fails a Diverged whose branch's last commit is the log's or its base; spanning_wals profile (writes 18, commits 4) holds 200 seeds and finds the bug at seed 170 in 27 s with wal_continues off (sim::logs_need_to_continue_a_segment_landed_alone; the wals profile needed 1060 s). The sim's 200 seeds don't reach the live path (a panicking mutant passed): the store test covers it. specs/bucket: rule walContinues, Opened models the fix, BucketSpans's Can has spanOn: holds, 5,675,698 distinct states, 122 s; mutations/NoWalContinues (1 host, Segs 4) violates NoErrors; every mutation's Rules list has walContinues; scripts/check-tla.sh: all 81 green. TASK-338's uploader grouping on TASK-310's branch (first segment landed last) was a workaround for this, dropped there now. Crash tests 30/30 on ubu1 (round 45 had failed them).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Wal::rebase keeps a landed segment ending the branch's last commit and going on with the next, restamped with its continuation alone; the store reads that commit's extents where they landed; the sim fails false divergences; specs/bucket's BucketSpans covers spanOn and holds.
<!-- SECTION:FINAL_SUMMARY:END -->
