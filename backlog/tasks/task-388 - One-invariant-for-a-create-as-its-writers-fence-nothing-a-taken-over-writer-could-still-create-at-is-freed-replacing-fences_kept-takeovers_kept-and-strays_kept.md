---
id: TASK-388
title: >-
  One invariant for a create as its writer's fence: nothing a taken-over writer
  could still create at is freed, replacing fences_kept, takeovers_kept and
  strays_kept
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 19:26'
updated_date: '2026-11-13 01:48'
labels:
  - extent_store
  - collector
  - correctness
dependencies: []
priority: high
ordinal: 765000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Since TASK-371 a durable commit is one bucket round trip: a writer counts its successful create as its fence and acknowledges without reading its lease. That is safe only while every position a taken-over writer might still create at stays occupied. Three collector rules now keep it so (fences_kept, takeovers_kept from TASK-371; strays_kept from TASK-383), each found only after the simulation lost an acknowledged commit (TASK-377's count, TASK-380's re-read is separate, TASK-383). User decision 2026-11-12: keep the one-round-trip design, but state the safety once instead of by cases. Write the invariant in specs/bucket as a single property (no object at a position some live-or-taken-over writer could still create at is ever deleted, until no such writer can exist) and derive the collector's keep-set from one function that computes those positions, replacing the three rules; doc-7 states the rule once. Then hunt for a fourth hole on purpose: models with more takeovers and writers than today's configs (tractable via Can flags as TASK-380 did), a mutation per way the keep-set could shrink, and a long simulation soak (SIM_SOAK high, the recreated/takeover profiles) on ubu1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 specs/bucket states the invariant once, the collector computes its keep-set from one function, and the three case rules are gone (or are shown to be that function's cases)
- [x] #2 Models with at least one more takeover and writer than today's configs hold, and a mutation per way of shrinking the keep-set finds a lost acknowledged commit
- [x] #3 A simulation soak of 100,000 seeds across the takeover profiles loses nothing
- [x] #4 doc-7 states the rule once, with why it is enough
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. State records facts: fences and strays (void/late) apart; one function collector::pinned(state, lease) computes the keep-set; rules pins_fences, pins_old_fences, pins_void_strays, pins_late_strays, pins_takings are its cases, replacing fences_kept/takeovers_kept/strays_kept. FORMAT 6 -> 7. 2. specs/bucket: one Pinned operator in GcFinish; invariant CreatesNeverFreed (history var freed kept by Next): the first place a live writer (taken over or taking over) may still create at and acknowledge was never freed. 3. Mutation per case (NoPins*), each finding NoAckedLost; hunt models with one takeover and one writer more (BucketTakeovers: 3 hosts, 4 takings). 4. Direct tests per case (rule-off twins), sim test_needs per case. 5. 100,000-seed soak of the takeover profiles on ubu1 (fxjob --mem 8G). 6. doc-7 states the rule once.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done in 448bfb5bc (+ f926e1488, TASK-390's unused imports that had main's clippy red). Invariant: specs/bucket CreatesNeverFreed (history var freed kept by Next; Creatable = in-flight creates, an opened writer's un-uploaded WAL, a fencing writer's fence while the lease is its), in every designed model. One function: collector::pinned(state, lease) -> Pinned::All while a taking is in flight, else fences (all epochs) + strays (void, late); State gained strays: Vec<Stray{seq, late}>; FORMAT 7. The three rules are its cases: pins_fences, pins_old_fences, pins_void_strays, pins_late_strays, pins_takings. Model fidelity found on the way: BucketStrays at 4 segments never modelled the void stray (the third writer, on the first's host, had no room to put its WAL after its fence, so it stopped before checkpointing); 6 segments do. Also CreatesNeverFreed first flagged a fencing writer whose lease had moved on, which can never acknowledge (epochSequences stops it): Creatable counts a fence only while the lease is the writer's. Numbers: BucketTakeovers (3 hosts, 4 takings, 5 segs, 2 commits, landInOrder) holds 207,375,573 states (1 h 47 min); BucketStrays (6 segs) 31,379,038; all other 58 models and mutations pass, each NoPins* finding NoAckedLost (NoPinsVoidStrays 183 s, NoPinsOldFences 48 s, NoPinsLateStrays 14 s, NoPinsFences, NoPinsTakings 1-2 s); NoNewestCheckpoint keeps pinsTakings out, as it kept takeoversKept. Sim: 100,000 seeds (5 takeover profiles x 200 x SIM_SOAK=100) on ubu1 under fxjob --mem 8G, 0 failures; sim searches find each case (seed 453 void, 843 late, 0 old fences, takings and fences as before). Linux: ubu1 test-host.sh (clippy, workspace 2547/2547, sim 55/55) on the scratch commit; macOS 428/428 + sim 55/55 after rebasing onto TASK-390. Guide: one-writer evidence names the five mutations; pnpm build + test pass. No fourth hole found.

Main CI 43345172428 on f926e1488: all 17 jobs green (macOS and Linux workspace, sim, guide, clippy).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One property and one function replace fences_kept, takeovers_kept and strays_kept. specs/bucket's CreatesNeverFreed: the first place a live writer, taken over or taking over, may still create at and acknowledge was never freed. collector::pinned computes the keep-set from State::fences and State::strays (void/late), plus everything while a taking is in flight. Checkpoint FORMAT 7. Its five cases are rules with a TLA+ mutation each (all find NoAckedLost), direct tests, and sim searches. Hunt: BucketTakeovers (3 hosts, 4 takings) holds at 207M states, BucketStrays at 6 segments at 31M, all other models pass, and 100,000 sim seeds on ubu1 lost nothing. No fourth hole found. doc-7 states the rule once. Main CI green.
<!-- SECTION:FINAL_SUMMARY:END -->
