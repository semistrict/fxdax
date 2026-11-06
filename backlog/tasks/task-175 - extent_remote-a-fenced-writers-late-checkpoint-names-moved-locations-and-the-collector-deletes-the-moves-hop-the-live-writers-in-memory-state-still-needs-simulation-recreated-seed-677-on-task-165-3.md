---
id: TASK-175
title: >-
  extent_remote: a fenced writer's late checkpoint names moved locations, and
  the collector deletes the moves hop the live writer's in-memory state still
  needs (simulation, recreated, seed 677 on task-165-3)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 07:46'
updated_date: '2026-11-06 06:56'
labels:
  - remote
  - collector
dependencies: []
priority: high
type: bug
ordinal: 633000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the simulation's soak on branch task-165-3 (SIM_SOAK=10 of stale_creates_landing_past_deletions, profile recreated): seed 677, 'branch 2's writer: lost: ExtentId { branch: 2, sequence: 1<<40 } in segments/2/1'. Main passes seeds 0..20000 of that profile; on task-165-3 seed 677 is the only one of 0..20000 that fails. The branch changes the batches' footers (TASK-165.3 step 1), which moves every run's requests and so its trajectory; nothing it changes touches checkpoints, moves or the collector's hops.

Trace (segments of branch 2): writer e1's segments 0 and 1 (extents 1<<40 and 1<<40|1 in 1); writers e2 and e3 fence at 2 and 3; the collector compacts segments/2/1 into moves/segments/2/1; e3 writes segment 4; then checkpoints/2/2 lands: the fenced e2's, through its fence, whose locations Writer::checkpoint followed through the moves to moves/segments/2/1. The collector recovers branch 2 from that newest checkpoint plus segments 3 and 4. Its live states name moves/segments/2/1, not segments/2/1, so it compacts moves/segments/2/1 into moves/moves/segments/2/1 and deletes segments/2/1, then moves/segments/2/1. The live writer e3's in-memory state still names segments/2/1, and its read finds neither that object nor the hop's map (moves/segments/2/1): lost.

The hole: hops are kept for the locations the bucket's live states name. A checkpoint from a fenced writer, newer by segment number than anything the live writer checkpointed, can name locations ahead of the live writer's own state.

Reproduce: SIM_SEED=677 SIM_PROFILE=recreated cargo nextest run -p extent_remote --run-ignored only sim::one (on task-165-3).
Options: refuse a checkpoint by a writer whose epoch is below the last segment applied in its range (or have recovery start only from checkpoints of the epoch of the branch's last segment it covers); or keep the hops of moves a grace period after the collector last saw a live state name their source. TLA+ (TASK-165.2) should model it before the fix.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A seed of the simulation that reproduces it on main, or a test pinning the scenario, red before the fix
- [x] #2 The collector never deletes a moves object a live writer's state may still need as a hop; the simulation soaks clean (SIM_SOAK=100 of every holds test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Root cause: the collector keeps the moves on the way to an extent only from where the branch's state (its newest checkpoint) says it is. A checkpoint landing after the branch's live writer recovered (a fenced writer's, or the writer before's while the next one recovers) can name locations further along the moves than the writer's memory: the collector then deletes the moves the writer's reads need.
2. Direct test (collector.rs): writer A checkpoints, the collector compacts, B takes over, A's late checkpoint lands, the collector compacts the moves and deletes; B's read of the extent is lost. Red on main and on this branch.
3. Rule (Rules::ways_from_seals): the collector trusts a branch's state to be no further along the moves than its writer's only when its newest checkpoint is by the writer holding the branch's lease, read after the checkpoints were listed; otherwise (another's checkpoint, or none: an inherited state), it keeps every moves object on the way from where each extent was sealed (layout::Kind::Moves' from, back to the seal). Recovered says the epoch of its checkpoint's writer.
4. Writer::checkpoint adopts the locations it records before it creates the checkpoint, moving only forward along each extent's moves, so its memory is never behind its own checkpoint.
5. Simulation: oracle (check_garbage) needs the same moves; test_needs for the rule; the soak of every holds test 0..20000.
6. doc-7 Garbage collection, OPEN_QUESTIONS.md decision, the TLA+ rule for TASK-165.2.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause. The collector kept a moves object as a hop only on the way from where the state it reads (the newest checkpoint and the footers after) names an extent. A checkpoint that lands after the branch's live writer recovered it can name extents further along their moves than the writer does: in seed 677 the writer taken over (epoch 1, its last acknowledged segment 2) checkpointed late, past the checkpoint the live writer (epoch 3) recovered from, following segments/2/1 to moves/segments/2/1. The pass then compacted moves/segments/2/1 in turn and deleted segments/2/1 (no longer named) and moves/segments/2/1 (named, compacted a grace ago, no hop): the writer's read of segments/2/1 found neither the object nor its moves. The same holds for a fork's inherited extents (its writer read the parent's state as it was then, and a parent's checkpoint up to the fork can land later), and for a writer's own checkpoint between its landing and the writer taking its moves, which it did once the create was answered (a retried ambiguous create is a window of round trips).

Main is affected: writers_read_past_checkpoints_landing_after_they_took_over is red on main at 13db5c67c with the same Lost. Not by a seed: main's recreated soak passes 0..20000; task-165-3's batch footers moved the trajectories onto it.

Fix (9ca6a6332). (1) A state's ways to its extents start where they were sealed, every moves on them kept (layout::Kind::Moves' from, back to the segment or extent), unless the state's checkpoint is by the writer holding the branch's lease, read after the recovery listed the checkpoints (Recovered::checkpointer; collector::behind): a writer taking the branch after that read recovers from that checkpoint or a later one. Inherited extents always. (2) Writer::checkpoint takes its moves before it creates the checkpoint, only forward along the way (layout::further). Rules ways_from_seals and moves_taken_first.

Blind spot: the simulation's writer-read check ran, but reaching it needs two things together, a checkpoint landing after a takeover and a second compaction of the same object within the live writer's life; the oracle (check_garbage) only knew ways forward. Tests added: the direct test above and writers_name_what_their_checkpoints_move_before_they_land (a hook runs passes as the checkpoint's create takes effect), each also with its rule off (Lost); late_checkpoints_need_ways_from_seals (the simulation finds seed 677 with the rule off); the garbage oracle needs the moves back to seals for branches whose state isn't their lease holder's checkpoint.

Looked around: host reads go through Writer::location or a point's fixed state (fxdax::remote::Fetcher), both covered (a point inherits, so always back to seals); fetchers looking extents up by checkpoint tables rely on the grace period, as decided before; open_recovered / recover_on restart on a newer checkpoint. The soak at SIM_SOAK=100 (TASK-165.3 ran 10) found four more holes, not of this class, three of them before this change: TASK-177 (forks seed 210, crashes seed 12424), TASK-178 (stale seed 3953, main too), TASK-179 (bases seed 9030), TASK-180 (restores seed 13797); main's soak also fails bases seed 1775 (lost: branches/2). Fixed in this batch, each its own commit.

Structure: the class is the collector's view of a branch being ahead, along moves, of what a live reader holds. Making it impossible exactly would need checkpoints to count only once a later footer names them (monotone), a format change with costs on every recovery while the newest waits; the rule kept is conservative and costs a lease GET per checkpointed branch per pass. The tempting fix of the fence naming the checkpoint its writer recovered from is unsound (dead-ends/fences-naming-their-checkpoints.md). Recorded in OPEN_QUESTIONS.md and doc-7 (Checkpoints; Garbage collection, step 5).

For the TLA+ spec (TASK-165.2): model each extent's way (its seal, then moves^1..moves^k, each object existing or not), each writer's own location of it (from the checkpoint it recovered from, moved forward only by its own checkpoints), checkpoints written by any writer as of its last acknowledged segment and landing at any later time (after a later writer took and fenced), the lease as (epoch, holder), and passes that compact (create moves^{k+1}) and delete. Safety: for every writer whose lease is still its taking and every extent it holds at moves^j, moves^j or moves^{j+1} exists (its read finds the extent). The rule that makes it hold: a pass keeps every moves^i, 1 <= i <= k, of each extent its state names at moves^k, unless that state's checkpoint was written by the lease's holder, the lease read after the checkpoint was listed; and a writer takes its checkpoint's moves before the checkpoint can land. With the first off, TLC should find seed 677's trace: e3 takes and recovers from C1, a pass compacts X, e1's late C2 names moves/X, a pass compacts moves/X and deletes X and moves/X, e3 reads X.

Verified 2026-11-05 at 348d941e8. Simulation soak, release, SIM_SOAK=100 of every holds test (0..20000 each, everything 0..30000): 14/14 pass (it found seeds 210, 3953, 9030, 12424 and 13797 before TASK-177 to 180; main at 13db5c67c fails bases 1775 and stale 3953 at the same soak). Rule tests: every test_needs passes, collection_needs_records_listed_after_states on its own profile now (348d941e8). macOS FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh --no-fail-fast: workspace 2078/2079, the one failure that rule test before 348d941e8, passing alone after; shuttle part (run alone, as the script stops at the workspace's failure) all green. Linux (Docker, OrbStack, rust:1, arm64): cargo nextest run -p extent_remote -p extent_store 406/407, the same rule test, passing there after 348d941e8 (176 s). scripts/upstream-changes.py --check passes. Not run: GCE (scripts/test-gce.sh).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**Late checkpoints: the collector's ways to extents start at their seals (TASK-175, 2026-11-05)**

A checkpoint landing after a branch's writer recovered it (the writer it took the branch from, fenced and unaware, checkpointing late) names extents further along their moves than the writer does. The collector kept the moves only from where the state it read names them, compacted them in turn and deleted them: the writer lost the extents. Reachable on main (a direct test, red at 13db5c67c); a fork's inherited extents, and a writer's own checkpoint before it took its moves, had the same hole. Decided:
- **The way to an extent starts where it was sealed, every moves on it kept, unless the branch's newest checkpoint is by the writer holding its lease**, read after the checkpoints were listed; inherited extents' always (doc-7, Garbage collection, step 5). *Cost:* a lease GET per checkpointed branch per pass; moves compacted in turn stay while a branch's newest checkpoint isn't its lease holder's (until that writer's next one) or while forks without checkpoints of their own inherit through them.
- **A writer names what its checkpoint moves where it went before it writes it**, only forward along the way.
*Instead:* (1) a checkpoint counts only once a later footer names it: a format change, a fence rebasing on what it finds, recoveries reading an older checkpoint while the newest waits, forks no longer starting from the checkpoint at their fork; (2) a fence naming the checkpoint its writer recovered from: unsound (`dead-ends/fences-naming-their-checkpoints.md`); (3) keeping a moves a grace period after a pass last saw a state name what it is of: time, which doc-7 says correctness doesn't depend on.
*Undoing it:* `Rules::ways_from_seals` and `moves_taken_first`'s code (collector.rs's `behind`, writer.rs's `take_moves`).

The soak this was verified with, SIM_SOAK=100 of every holds test (TASK-165.3 had run 10), found four more holes, on main too: main's soak fails stale_placements seed 3953 (TASK-178's, the same failure) and bases_deleted_under_live_forks seed 1775 ("collector: lost: branches/2", TASK-177's kind). Each fixed as the protocol's other rules go, rather than by a structure of its own:
- **A recovery that found no checkpoint of a branch, finding what it inherited instead lost, starts again if one has come** (TASK-177), as for a segment gone. *Instead:* keep a deleted parent's record while any child might recover from it: no bound on that.
- **A deletion whose recovery of the branch fails, the record deleted since, is done** (TASK-178), as the collector takes it.
- **A writer's lease is its taking, epoch and holder** (TASK-179). *Instead:* never remove a deleted branch's lease (an object per branch ever deleted), or take a missing lease only for a record read after it.
- **A pass fenced by a later one stops without failing where it finds something lost** (TASK-180), as it stops before deleting. *Instead:* read the records again before recovering what a relisting found: narrower, and every relisting a round of GETs.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A checkpoint landing after a branch's live writer recovered it (a fenced writer's, unaware) names extents further along their moves than the writer does, and the collector kept moves only from there: it compacted them in turn and deleted the writer's way (seed 677). Reachable on main (direct test red at 13db5c67c). Fixed in the protocol (9ca6a6332): ways start at extents' seals, all moves kept, unless the newest checkpoint is by the lease's holder (read after listing); inherited extents always; writers take their checkpoint's moves before writing it. Rules ways_from_seals and moves_taken_first, each with direct tests red with the rule off, and a simulation rule test (seed 677). The SIM_SOAK=100 soak used to verify it found four more holes, fixed alongside: TASK-177, 178, 179, 180. doc-7, OPEN_QUESTIONS.md, a dead end. Verified by the soak, macOS workspace and shuttle, Linux in Docker; GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
