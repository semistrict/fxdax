---
id: TASK-383
title: >-
  Simulation: stale_creates_landing_past_deletions seed 453 (SIM_SOAK=5) loses
  branch 2's acknowledged commits 27-29
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 12:39'
updated_date: '2026-11-12 14:13'
labels:
  - bug
  - simulation
  - collector
dependencies: []
priority: high
type: bug
ordinal: 762000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-12 by TASK-380's soak, once seeds replay (16b9f306a): extent_store sim profile recreated (stale_creates_landing_past_deletions), seed 453, which only SIM_SOAK>=3 reaches (CI runs 0..200). Deterministic: SIM_SEED=453 SIM_PROFILE=recreated cargo nextest run --cargo-profile sim -p extent_store -E 'test(=sim::one)' --run-ignored only fails every run with 'branch 2 lost commit 27/28/29, acknowledged'; SIM_TRACE=1 prints each bucket request. Not TASK-380's rule: it fails with SIM_OFF=deletions_reread too. Candidates: TASK-371's acknowledgement without a lease read (fences_kept, takeovers_kept), as the profile holds stale creates past takeovers and compacts.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The seed's loss understood and its rule or code fixed, with a direct test that loses the commits without the fix
- [ ] #2 The profile passes at SIM_SOAK=20
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Replay seed 453 with SIM_TRACE's request log; find who lands or deletes what loses commits 27-29. 2. Check specs/bucket for the case; model it at tractable bounds. 3. Fix as a rule (code + TLA+), direct tests that lose the commits without it. 4. Soak: 10,000 seeds of the profile, every sim test at SIM_SOAK=5, every TLA+ model and mutation. 5. Seed 453 always run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause (seed 453, recreated): writer 1 (epoch 1) had segments 1 and 2 in flight; writer 2 fenced at 1, so writer 1's segment 2 landed void. Writer 3 recovered past it (void, skipped) and fenced at 3, checkpointed at 3; the collector deleted segment 2, covered, being neither applied nor a fence. Writer 2's create of its segment 2 (commits 27-29), held by the profile until an object at its key was deleted, then landed in the hole, below writer 3's fence, and was acknowledged: since TASK-371 a writer's creates are its fencing, no lease read. fences_kept kept the successor's fence, but the writer taken over meets a stray first.

Model: specs/bucket allowed it but its configs never had three takings with checkpoints and a pass (MaxTakes 2). New designed model BucketStrays (1 branch, 2 hosts, 4 segments, MaxTakes 3, MaxCommits 3) found, with void segments alone kept, a second stray: an earlier writer's late fence, made after its successor's fence, which it follows (applied, lower epoch, not a fence), deleted, the successor's create landing there.

Fix: rule strays_kept (straysKept): State::fences also counts strays, every segment read not of the epoch of the fence before it (void, or applied of a lower epoch); the collector keeps them with the fences. Checkpoints carry them: FORMAT 5 -> 6. BucketStrays holds (7,117,696 states); mutation NoStraysKept finds NoAckedLost (10 s). Direct tests: collector::tests writers_taken_over_find_void_segments_before_their_successors_fences_kept and ..._find_late_fences_..., each with a rule-off twin that acknowledges a commit the branch lacks. sim: takeovers_need_strays_kept finds seed 453 with the rule off; stale_creates_landing_past_deletions always runs seed 453 (test_holds! takes found seeds).

Blind spot: TLA+ bounds (two takings) and seeds that didn't replay; the sim's 200 seeds per profile miss 1-in-~1000 cases (seed 453 only at SIM_SOAK>=3). Structure: TASK-371 made correctness depend on what the collector keeps below fences (fences_kept, takeovers_kept, now strays_kept): three rules for one invariant, 'every position a live writer taken over may create at stays occupied'. Noted in OPEN_QUESTIONS.md for the user.
<!-- SECTION:NOTES:END -->
