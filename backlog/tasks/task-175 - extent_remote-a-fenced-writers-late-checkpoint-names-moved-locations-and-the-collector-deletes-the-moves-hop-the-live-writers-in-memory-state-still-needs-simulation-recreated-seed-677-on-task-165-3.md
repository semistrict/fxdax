---
id: TASK-175
title: >-
  extent_remote: a fenced writer's late checkpoint names moved locations, and
  the collector deletes the moves hop the live writer's in-memory state still
  needs (simulation, recreated, seed 677 on task-165-3)
status: To Do
assignee: []
created_date: '2026-11-05 07:46'
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
- [ ] #1 A seed of the simulation that reproduces it on main, or a test pinning the scenario, red before the fix
- [ ] #2 The collector never deletes a moves object a live writer's state may still need as a hop; the simulation soaks clean (SIM_SOAK=100 of every holds test)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
