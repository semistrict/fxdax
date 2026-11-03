---
id: TASK-149.5
title: 'Every crash of a short sequence, not a sample'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 506000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Random cuts took minutes to reach several of the 2026-10-31 bugs. For short sequences every barrier and every subset of lost writes can be tried, which is complete up to a bound, as ALICE and CrashMonkey do.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 For sequences up to a bound, every barrier and every subset of the writes since the last, up to a limit, is replayed and checked
- [x] #2 Reintroducing the super-block offsets bug fails
- [x] #3 The bounded run takes under a few minutes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Crashes::every: at each barrier, every subset of what it may lose (a free, or an extent's writes), up to a bound, and past it nothing, all, each alone, all but each, counted.
2. Short sequences in the model test, with compactions on their own and journal extensions from a small reclaim size and chunk, and fills racing a compaction, with a store flushing.
3. Show the super-block offsets bug, put back, fails. Time the bounded run.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Bound: sequences of up to 6 operations; at each barrier every subset of up to 6 things (a free, or an extent's writes); past that, nothing, all, each alone, all but each. About 18% of barriers go past it. FXDAX_MODEL_EVERY_CUT=120 takes under 3.5 minutes, about 8,000 cuts. The super-block offsets bug put back fails every_cut_of_a_store_flushed_around_a_super_block in 3 s. That bug needs an object's flush to end in the window: found force_compact overlapping the flush task's compaction (fixed), after which only a store's own flush reaches it, so the test flushes the store. See dead-ends/theory-commits-racing-a-super-block-lose-the-journal.md.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every barrier and subset up to a stated bound, in under 3.5 minutes, failing with the super-block bug back; found and fixed overlapping compactions that left no readable super-block.
<!-- SECTION:FINAL_SUMMARY:END -->
