---
id: TASK-165.2
title: >-
  TLA+ of doc-7 with the host's WAL: leases, fences, segments, forks, points,
  the collector; a mutation per rule
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 03:16'
updated_date: '2026-11-05 13:26'
labels: []
dependencies: []
documentation:
  - doc-14
parent_task_id: TASK-165
priority: high
ordinal: 624000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-14, step 2; TASK-165 AC #3; covers TASK-44.5 AC #1. The simulation samples interleavings and missed TASK-79.18 and 79.3 until after the fact; TLC exhausts small cases. The restructure adds protocol (a WAL rebased after a takeover), so the spec comes before that code: a design bug is cheapest found here.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 specs/bucket/Bucket.tla models leases with fencing tokens, the fence, dense create-only segments linked by after, acknowledgement after a lease read, hosts crashing with or without their WAL and rebasing it after the fence, forks recorded after acknowledgement, points, and a collector that lists first
- [x] #2 Invariants hold at small bounds (2 hosts, 2 branches, 4 segments, 1 collector): no acknowledged commit lost, each history a path of its own commits, no ID minted twice, nothing a live branch needs deleted
- [x] #3 One mutation per field of extent_remote's Rules, plus wal_follows and wal_after_fence, each giving a counterexample, run by make check-tla (scripts/check-tla.sh)
- [x] #4 doc-7 and doc-14 name the spec, and each rule names its mutation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read doc-7, doc-14, extent_remote's Rules, the simulation's rule tests (run with --no-capture: what each rule's absence breaks), and specs/dax-cow and specs/follower for structure.
2. specs/bucket/Bucket.tla: the bucket (segments, own extents, checkpoints, records, leases, moves, creates in flight), writers (lease, recovery in steps, fence, lease check, wal::rebase, commits into the WAL, uploads, acks after a lease read, record_forks, checkpoints), hosts crashing with or without their WAL, placements, deletions, failed forks' deletions, restores of points, IDs taken by other branches, one collector's passes. Every rule a member of Rules.
3. Invariants: NothingNeededGone, HistoriesOwn, NoAckedLost, NoIdTwice, and the simulation's other checks (PointsUnwritten, LeasesCurrent, DeletedOnlyByDeletes, OneMaker, NoErrors).
4. As-designed models at 2 hosts, 2 branches, 4 segments, 1 collector (several, each a part of the steps), and points with 3 branches.
5. specs/bucket/mutations: one per rule (21 + walFollows, walAfterFence), each the smallest model finding its counterexample; scripts/check-tla.sh runs them.
6. Design bugs found: recorded in the task, doc-14, OPEN_QUESTIONS.md; collector bugs as tasks.
7. doc-7 and doc-14 name the spec; each rule names its mutation (spec header, extent_remote's Rules docs).
8. Verify: scripts/check-tla.sh (all specs and traces), timed; scripts tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model (specs/bucket/Bucket.tla): the bucket (segments with one commit, an extent sealed or of its own, a free, next sequence, a fork; own extents at keys naming epoch and segment; checkpoints; records; leases with tokens; moves and moves of moves), creates in flight landing late, writers in steps (lease, recovery start, segments then checkpoints again, fence one create a step, lease check and chain, wal::rebase, commits into the WAL, uploads, acks after a lease read, record_forks, checkpoints following moves), crashes keeping or losing the WAL, placements, deletions (end_lease, then the record and the children), failed forks' deletions (delete_fork's claim), restores of points (fork_point's three steps), IDs taken by other branches, one collector (listing and records, abandon, recover_live, relist, compaction, deletion). Can gates each kind of step so that models check parts of it.
Invariants: NothingNeededGone, HistoriesOwn (own path from the record's base, each commit applied once, holding what the commits made), NoAckedLost (the bucket's history and the current writer's guest), NoIdTwice, and the simulation's other checks: PointsUnwritten, LeasesCurrent, DeletedOnlyByDeletes, OneMaker, NoErrors.
How each rule's absence breaks was read off the simulation first (its 21 test_needs, --no-capture): e.g. acks_after_lease's seed 596 needs a checkpoint and a pass deleting the next writer's fence before the stale segment lands; fence's needs the collector.
Design bug in doc-14 (found by a model with commits, crash keeping the WAL and checkpoints, all rules): rebase by segments declares a WAL diverged when a later writer's checkpoint covers its landed segments (trace: h1 uploads seg 1 and crashes with its WAL; h2 recovers seg 1, fences, checkpoints; h1 retakes from the checkpoint: no footer of seg 1 read, so diverged though the branch holds c1), and when only an empty fence came after it. Fix chosen: rebase by commits (the WAL's base and commits against the branch's last commit after the fence). doc-14 step 4 and OPEN_QUESTIONS.md # Decisions say so. walAfterFence's counterexample under it: the WAL's own segment landing between recovery and the fence, applied twice.
Collector bugs found, modelled fixed, filed: TASK-173 (BucketPointsDeep: a child recovered by its parent's state is not recovered again by a record of another origin found on relisting), TASK-174 (BucketForks: a deleted root's record dropped, made again as a fork's child by the parent's slow writer). TASK-175 (found by the simulation on task-165-3) needed moves of moves, which the model lacked: added (reads and checkpoints follow moves' maps hop by hop, collector.rs's hops and its rule for deleting moves); mutations/Task175.cfg reproduces its trace exactly in 29 steps and must find it until its fix is a rule; the models as designed compact no moves until then.
Step merges (open with recovery start, the fence's last create with the lease check and rebase, the collector's listing with its records) and host symmetry cut the state spaces 3-10x.

TASK-175 (simulation, task-165-3): the model lacked moves of moves; added, and mutations/Task175.cfg reproduces its trace (expect NothingNeededGone, all rules), to become the mutation of TASK-175's rule (its plan: ways_from_seals) once fixed; the models as designed leave compactMoves out until then.
Verification: scripts/check-tla.sh, all specs and the follower traces, alone on the machine: exit 0 in 6m15s (bucket's 10 models as designed and 24 mutations about 4 min of it; BucketCompaction since cut to 3 segments, 30s); every mutation PASS with its invariant, every model as designed holds. FXDAX_TEST_PARTS="clippy scripts" scripts/test.sh: exit 0 (151 scripts tests, mutants checks). scripts/upstream-changes.py --check: exit 0. cargo check -p extent_remote, cargo fmt --check: clean (lib.rs's change is doc comments only).
DoD #1 not run: the only Rust change is doc comments on Rules, so the workspace and GCE runs were not repeated for it.

TASK-175 to 180's rules (2026-11-05, on land-165): waysFromSeals (collector::behind: a live state not from a checkpoint of the lease's epoch keeps every moves back to the seal), movesTakenFirst (a checkpoint's create now lands in a step of its own; the writer takes its moves before it, or, without the rule, once answered), leasesByTakings (leases are [epoch, token, holder], the holder the session's gen; "staleTake" lets a taking's first record read be from before the deletion, the record read again after the lease), passesFencedStop (gc is a pass per lease epoch, "overlap" lets them run at once; a pass reads records as listings find them, GcRecords, until none is new), TASK-177 under newestCheckpoint (a deletion's recovery lists checkpoints, then goes on in a step of its own, Recovering, restarting when one came since), TASK-178 as the spec rule deletionsDoneByOthers (no Rules field). Mutations: NoWaysFromSeals (Task175.cfg's trace), NoMovesTakenFirst, NoLeasesByTakings, NoPassesFencedStop, NoNewestCheckpointInherited, NoDeletionsDoneByOthers; models as designed compact moves again (BucketCompaction), BucketForks takes stale, BucketOverlap is new. Found TASK-183 (two deletions at once, a fork segment landing between their recoveries: the second records the child after a pass let go of its origin), modelled fixed as deletionsFence, mutation NoDeletionsFence (BucketForks's first counterexample once deletions recovered in two steps). Verified: scripts/check-tla.sh, all specs and traces, exit 0 in 1035s (bucket's as designed 835s of it: BucketOverlap 313s, BucketForks 238s; mutations 2s each but NoWaysFromSeals 57s, NoRecordsRelisted 17s, NoPassesFencedStop 16s). BucketPoints with overlap was cut, past 223M states. cargo check -p extent_remote clean (doc lines only).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/bucket/Bucket.tla: doc-7's protocol with doc-14's WAL in TLA+ (writers in steps with leases, tokens, fences, acks after a lease read, wal::rebase; crashes with or without the WAL; forks, failed forks' deletions, points and restores; placements; one collector with compaction and moves of moves). Invariants NothingNeededGone, HistoriesOwn, NoAckedLost, NoIdTwice and the simulation's other checks hold in ten models as designed (2 hosts, 2 branches, 4 segments, 1 collector; points at 3 branches). One mutation per Rules field plus wal_follows and wal_after_fence, each a counterexample, run by scripts/check-tla.sh. Found: doc-14's rebase by segments wrongly diverges WALs a checkpoint covers or only a fence passed (fixed in doc-14: by commits; OPEN_QUESTIONS decision); collector holes TASK-173 and TASK-174 (modelled fixed, filed); reproduces TASK-175 (mutations/Task175.cfg). doc-7, doc-14 and extent_remote's Rules name the spec and each rule's mutation. Verified by check-tla.sh (exit 0, 6m15s), the scripts tests and upstream-changes --check.
<!-- SECTION:FINAL_SUMMARY:END -->
