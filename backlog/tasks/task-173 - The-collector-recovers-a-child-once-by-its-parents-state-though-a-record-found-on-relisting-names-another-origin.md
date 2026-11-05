---
id: TASK-173
title: >-
  The collector recovers a child once, by its parent's state, though a record
  found on relisting names another origin
status: To Do
assignee: []
created_date: '2026-11-05 07:38'
labels: []
dependencies: []
priority: high
type: bug
ordinal: 633000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by specs/bucket's TLA+ (TASK-165.2), the model BucketPointsDeep, before its fix was modelled. A pass reads the records before a child has one; recovering its parent S it finds S's fork to child C in S's state, unrecorded, and recovers C as S's fork at that segment (forks_from_states), marking C taken. Meanwhile a restore of point P took the ID C for its own fork (fork_point: C's record names P's origin, S at an earlier segment), so S's fork to C is refused (ChildTaken). The pass, S recovered from a checkpoint of its own, lists the records again (records_relisted) and finds C's record, but recover_live queues only branches not in live.taken: C is never recovered by its record's origin. What S's state at the restore's segment needs (S's segments up to there, which S's checkpoint covers) is deleted, and C recovers from nothing: lost. Trace: S forks P (point) at 1 and a point to C at 2; C restored from P before S records it; P deleted; S checkpoints at 2 dropping P; a pass started before C's record recovers S, C by state, relists, deletes segments 0 and 1. crates/extent_remote/src/collector.rs, recover_live and Live::taken. The model keys recoveries by branch and origin (specs/bucket/Bucket.tla, RecoverAll), and its may_land asks every recovery of a branch.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test in collector.rs or the simulation that loses the restore's child with the code as it is, and passes after
- [ ] #2 recover_live recovers a branch again when a record names another origin than the one it was recovered by; may_land and what is named go by every recovery
- [ ] #3 The simulation's restores profile makes the case likely (a fork and a restore naming one child), and a seed finds it with the fix reverted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
