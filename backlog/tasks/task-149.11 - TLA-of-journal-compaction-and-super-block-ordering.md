---
id: TASK-149.11
title: 'TLA+ of journal, compaction and super-block ordering'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-01 06:13'
updated_date: '2026-11-08 04:33'
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
- [ ] #1 A spec covers commits, object flushes, device flushes and super-blocks
- [ ] #2 Its invariant fails if offsets are read after the flush
- [ ] #3 It passes on the code's order
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
