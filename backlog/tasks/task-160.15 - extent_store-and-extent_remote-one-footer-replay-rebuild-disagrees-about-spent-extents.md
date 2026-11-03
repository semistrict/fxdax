---
id: TASK-160.15
title: >-
  extent_store and extent_remote: one footer replay; rebuild disagrees about
  spent extents
status: To Do
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-03 18:50'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 586000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Footers are replayed three times (remote recovery, local rebuild, delta replay). rebuild (index.rs) spends every free of the branch's own extents, though a footer lists frees of extents freed while open, which the live store doesn't spend (10e457ebd's rule): unseen because Rebuilt has no spent field. next_sequence and continuity differ too. Checkpoints have two schemas. Explains TASK-150.6 patched in both crates, 79.1, 79.6. Proposal: extent_frame::segment::Replay with apply(&Footer, &mut impl Places).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Rebuilt reports spent; a test (write, free before the commit, commit, close, rebuild) agrees with the live store, red before
- [ ] #2 One replay used by both crates
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Rebuilt reports spent (Store::spent for the store's); the rebuild oracles (closed_state, the crash rounds) compare it.
2. Red test: write, free before the commit, commit, close, rebuild, compare with the live store.
3. Fix at the source: footers free only extents a commit held (layout::held_frees, shared with delta replay).
4. Unify the three replays only if clean; otherwise record why.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Divergence real. A footer's window listed frees of extents written and freed between commits (open, never held). Local rebuild spent them all; the live store doesn't (10e457ebd's rule). Red: rebuilding_spends_what_the_store_spends (rebuild spent 1..4, store 1), and closed_segments_hold_the_branchs_history once it compared spent (33, 36, 39 extra).
Fix (0a676729d): Window::commit frees only layout::held_frees(changes), the same rule a delta's replay now uses (refactor fa89824b0). Every footer free is then of an extent a commit held. Remote recovery is unchanged: it spends only frees of extents it held before the segment, which deliberately leaves an extent sealed and freed inside one segment writable on another host. Its comment is updated.

Blind spot: Rebuilt had no spent field, and the crash rounds rebuilt only right after a reopen, when the checkpoint written at opening covers the whole workload. The rounds now also rebuild before the reopen (rounds.rs rebuilds()), comparing epoch, extents, root, next_sequence and spent. That found a second divergence (e3b066e2c): the segment the newest checkpoint names, closed between commits after the checkpoint's commit, has its footer's last commit equal to the checkpoint's. Reaped, it was released at once, so rebuild, which anchors on it, stopped at the checkpoint (round 3: epoch 44 of 53). IndexLog::let_go now keeps it until a later checkpoint names another. Red before: the_segment_a_checkpoint_names_stays_until_another_does and the crash round. Both divergences showed only through rebuild, a test-only oracle of doc-7's 'history is in the segments': remote recovery already ignored the extra frees, and the uploader keeps segments until they are uploaded.

AC #2 (one replay) not done. The three differ on purpose: remote spends only extents it held before the segment, takes the max next_sequence, and has its own continuity rule and layers; local rebuild anchors on the checkpoint's segment; delta replay reads Changes, not footers. TASK-153.7/.8/.9 are changing the footer format in both crates now. Deferred (OPEN_QUESTIONS.md decision): unify after 153.x lands, choosing one rule for each difference.

AC #1 done and verified: rebuilding_spends_what_the_store_spends was red before (rebuild spent 1..4, store 1), now green, as are the crash rounds that compare spent (macOS; Linux workspace 2228/2228 at 30049162f). AC #2, one replay, is open (see the decision in OPEN_QUESTIONS.md), so back to To Do for it.
<!-- SECTION:NOTES:END -->
