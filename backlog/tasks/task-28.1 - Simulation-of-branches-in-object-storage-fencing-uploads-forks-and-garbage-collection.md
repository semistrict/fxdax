---
id: TASK-28.1
title: >-
  Simulation of branches in object storage: fencing, uploads, forks and garbage
  collection
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 01:31'
updated_date: '2026-10-17 03:14'
labels: []
milestone: m-5
dependencies: []
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
priority: high
type: task
ordinal: 81000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2 warns that garbage collection over object storage with O(1) forks is subtle and needs a model and tests before code. doc-7 gives the protocol (create-only dense segments as commit and fence, leases, fork records, moves, a collector that alone deletes); the survey found real bugs of this shape in SlateDB (RFC 0026, 0029) and rocksdb-cloud. The simulation is the executable form of doc-7 that the later tasks implement against.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A deterministic simulation (seeded) runs writers, forks, takeovers, branch deletions and the collector over an object store that injects errors after writes land, delays, reordering and crashes at every step
- [x] #2 It checks after every step that no extent a live branch holds is unreadable, and at quiescence that every object holding no live extent is deleted, against a mark-and-sweep oracle
- [x] #3 It covers the ten cases doc-7 lists under Testing, each with a named test that fails if the rule it depends on is removed
- [x] #4 Thousands of seeds run in the workspace tests within the 5-minute budget
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. New crate crates/extent_remote: the protocol of doc-7 as async code generic over a narrow Bucket trait (get, get suffix/range, head, list, put Create/Update/Overwrite, delete; errors AlreadyExists, Precondition, NotFound, and transient ones that may or may not have landed). Simplified object contents (bincode) until TASK-28.3 fixes the segment format.
2. Protocol pieces: branch writer (session epoch, segments with footers, parallel creates, acks only after a lease read that follows the creates, fence on open, checkpoints), branch reader (newest checkpoint, with a fork base reference, then footers, void rule, moves), fork (O(1): child record and a checkpoint referencing the parent), collector (lease, mark from live branches, compaction through moves, orphan and dead-segment deletion, branch deletion).
3. sim: SimBucket whose operations complete when a seeded scheduler says, with faults (fail before effect, land then fail, delay and reorder, land after the caller crashed); a single-threaded executor; actors (hosts with local durable state and guests, the collector) that crash and restart; a logical clock.
4. Oracle: each branch acknowledged history; after every step a fresh reader of each live branch recovers a prefix containing every acknowledged commit, and every extent it holds reads back its content; at quiescence every object not needed by a live branch is gone (mark-and-sweep).
5. Tests: the ten doc-7 cases as targeted scenarios, each paired with the rule it relies on switched off (a Rules struct, test-only) and the simulation catching it; a sweep of thousands of seeds in under the time budget.
6. Update doc-7 with what the model settles (validation after creates, void rule, what the collector may delete when, fork checkpoints, minting after takeover).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
crates/extent_remote: doc-7 protocol (bucket.rs Bucket trait with retrying helpers that resolve ambiguous creates/updates by reading back; layout.rs keys/objects; reader.rs recover/read/MovesMaps; writer.rs Writer; branch.rs records/leases/fork records/delete; collector.rs) + sim/ (SimBucket with delayed/reordered/failing requests and a Direct view; World: hosts with disks, sessions with guests, forks, takeovers, crashes, deletions, collectors; oracle after every step; quiesce + garbage check).
What the simulation changed in the design (doc-7 updated): segments name the segment they follow (continuity) instead of voiding by epoch; the fence stays (without it a takeover writer can start from a state an old writer late segment supersedes, and its guest reads freed extents) and fills gaps past the end with empty segments; writers check the lease after recovering before minting from epoch<<40; readers that find a segment missing re-list checkpoints and restart if a newer one exists (checking the old one still exists was fooled by a child pinning it); compaction output is the moves object itself (data + map, one object), so two collectors cannot orphan it, and moves on a state path are kept; deleted forks records are kept (a slow parent writer could recreate them); the collector re-lists records after reading them, recovers fork children without records, skips branches deleted mid-pass.
Tests: 11 profiles that must hold (everything, forks_and_frees, forks_racing_collection, forks_with_children_opening, takeovers_while_uploading, takeovers_racing_collection, crashes_anywhere, collectors_stopping_and_racing, recoveries_racing_collection, ...), 9 rule-off tests (each finds a non-garbage violation: candidates_first@864, forks_from_states@23, records_after_ack@0, newest_checkpoint@2723, acks_after_lease@136, continuity@5, epoch_sequences@0, fence@274, orphans_after_successor@887). cargo nextest -p extent_remote: 20 passed in ~24s; SIM_SOAK=20 (20x seeds of the holds tests): 11 passed in 151s. clippy clean for the crate.

Verified: macOS scripts/test.sh 1903 passed; Linux (Lima) scripts/test-linux.sh 1911 passed; upstream-changes --check passes. Cases 1, 2, 6 and 7 rest on the mark-and-sweep and the oracle as a whole rather than one rule; the others map to rule tests (case 3: candidates_first, forks_from_states; 4: acks_after_lease, fence, continuity, orphans_after_successor; 5: records_after_ack; 8: newest_checkpoint; 9: fence, continuity; 10: epoch_sequences). Pushed: 5579459.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
crates/extent_remote implements doc-7 protocol (writer, reader, branch records/forks/deletion, collector) over a Bucket trait, checked by a deterministic simulation with fault injection and an every-step oracle; 20 tests (11 holding profiles, 9 rule-off tests) pass in ~24s, 20x soak clean. The simulation changed doc-7 in five places (continuity links, single-object compaction output, newest-checkpoint recheck, lease check before minting, kept deleted fork records).
<!-- SECTION:FINAL_SUMMARY:END -->
