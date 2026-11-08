---
id: TASK-149.32
title: >-
  extent_remote's simulation: every acknowledged object exists, every upload,
  sync and durable commit woken
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 03:35'
updated_date: '2026-11-08 01:37'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 643000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Split from TASK-149.18, which the user took to guest/sim (2026-11-05): its invariants there are done. What is left is extent_remote's sim, as TASK-149.18 first said: it checks what its runs read back, so a loss shows only if a read reaches it, and a waiter never woken shows only as a hung run, if at all. Stating both as invariants, and generating own extents and branches of children, reaches what the audit traced by hand. The extent_store side needs a Shuttle or quiescence check of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 After each step, every object an acknowledged segment names exists, or is known lost by the rules
- [x] #2 Runs include children of their own branch uploading inherited extents of 4 MiB or more
- [x] #3 At quiescence no upload, sync or durable commit still waits
- [x] #4 The collector and upload bugs, put back, each fail it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Read the sim (crates/extent_remote/src/sim): its after-step check already asserts the objects each recovered branch read, and each extent's object or moves, exist; and a live writer's guest-held extents read back.
2. AC #3: count every task the sim spawns (World::tasks, by #[track_caller] spawn site); at quiescence, everything settled with nothing failing, a task still waiting is a wake lost (check_woken). Give each host's writers one upload budget of 2 PUTs, as extent_store's hosts share one, so that uploads wait on each other at all (unbounded, they never did).
3. AC #2: since TASK-165.7/165.9 no path uploads inherited extents (extent_store refuses another branch's extent, store.rs:950/970, segments.rs:837): what is left is children of the branch uploading own extents beside the collector, which forks_with_children_opening and the other fork profiles make.
4. AC #4: crates/extent_remote/mutants (mutants.sh --check in the scripts part): uploads-wake-no-waiter (a grant given back wakes no waiter) and own-extents-in-flight-collected (TASK-79.3/150.1's class: the collector deletes an own extent's object whose segment may still land).
5. Soak 1x, 5x, 25x, 100x; root-cause each violation.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08. check_woken at quiescence and per-host upload budgets added; the sim's after-step object check kept as AC #1 (recovered branches' touched objects and each extent's object or moves; live writers' guest-held extents read back). A stricter check I tried, every extent a live writer's acknowledged state names exists, failed at 25x (seed 4116 of everything): not a loss, the writer's state lags the bucket, a landed segment's free (commit 24, segment 5) unacknowledged yet, its old segment rightly reclaimed by the collector; dropped, a comment says why. AC #2 reread for doc-14 (no inherited uploads exist any more; children's own extents beside the collector are generated, and the collector mutant is killed on a child, branch 0x14 reading its parent 0x20's own extent, among others). The extent_store side of AC #3 (syncs, durable commits) is extent_store's Shuttle tests' (uploading_during_flushes, flushes_in_flight_keep_their_commits_pending and others), whose deadlock detection finds a waiter left blocked; no extent_store change, as asked.
Runs (macOS, profile sim): sim part 48 of 48; holds profiles at SIM_SOAK 5 (1,000 seeds each), 25 (5,000) and 100 (20,000 each, 400,000 runs): no violation after the writer-state check went; mutants.sh: uploads-wake-no-waiter killed by sim::everything (seed 0, uploads still waiting at world.rs:577), own-extents-in-flight-collected killed by sim::forks_racing_collection (seed 4, an acknowledged extent's object gone), and also by collectors_stopping_and_racing, everything and forks_and_frees; extent_remote's other tests 187 of 187; scripts' tests 194 of 194; clippy's disallowed methods; upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->
