---
id: TASK-149.11
title: 'TLA+ of journal, compaction and super-block ordering'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-08 06:25'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 512000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No spec states that a super-block claims only what its journal flushed, which is the invariant the worst bug of 2026-10-31 broke, and a few lines to write.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A spec covers commits, object flushes, device flushes and super-blocks
- [x] #2 Its invariant fails if offsets are read after the flush
- [x] #3 It passes on the code's order
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. specs/journal/Journal.tla: commits, objects' BeginFlush/EndFlush with journal_checkpoints' old and current offsets, device flushes as a durable prefix, write_super_block's pad, flush and save; invariants NoLostMutation (replay from the super-block's offsets loses no durable mutation) and SuperBlockFlushed.
2. Rules offsetsAtPad and flushBeforeSave; mutations NoOffsetsAtPad (offsets read after the flush) and NoFlushBeforeSave, each expecting its violation.
3. check-tla.sh and README list it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-07: land-h12 944600616, CI 42447165155 (CI doesn't run check-tla; run here): scripts/check-tla.sh journal/Journal holds, 1,548,717 states in 4 s; journal/NoOffsetsAtPad violates NoLostMutation (7 states: a mutation of a, a's BeginFlush, the pad at 3, a's EndFlush at 3, the flush durable to 2, the save: the mutation lost, as 2026-10-31's bug); journal/NoFlushBeforeSave violates SuperBlockFlushed. journal.rs's comment on the read points at the spec.

2026-11-08: on main as 722ce0723 (land-all4), CI 42449438764 (land-all4, on main as 722ce0723) green: test (workspace) on Linux, macos (workspace), guest-sim, guest-sim-sweep, scripts. CI runs no TLA+; check-tla.sh run here (Journal holds, both mutations violate as expected). DoD 3: journal.rs's comment only, pointing at the spec.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/journal models commits, objects' BeginFlush/EndFlush with journal_checkpoints' offsets, device flushes and write_super_block's pad/flush/save; NoLostMutation and SuperBlockFlushed hold on the code's order (1.5M states), NoOffsetsAtPad reproduces the 2026-10-31 bug in 7 states, NoFlushBeforeSave a super-block past its journal. Verified with scripts/check-tla.sh; on main via 722ce0723, CI 42449438764.
<!-- SECTION:FINAL_SUMMARY:END -->
