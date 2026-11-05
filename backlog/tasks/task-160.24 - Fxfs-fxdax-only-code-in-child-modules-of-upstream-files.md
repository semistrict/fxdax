---
id: TASK-160.24
title: 'Fxfs: fxdax-only code in child modules of upstream files'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 03:05'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 595000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
data_object_handle.rs was +2,596/-141 against upstream at the review, 1,637 lines of it pure additions (write_many, extents_in, txn_map_write, clones, zero_in_commits); volume.rs +1,197, 856 pure (snapshots, bases). Proposal: fxdax's additions in child modules (data_object_handle/{dax,write_many,reflink}.rs and volume/{snapshot,base}.rs), impl blocks, so that the upstream diff of those files shrinks. After TASK-153.5 and 153.12.

At 9f81f6018, writable DAX's deletion (37c331a6c) took txn_map_write and DaxWrite out of data_object_handle.rs (889 lines removed, 312 added), so the figures above are high by about that. Its additions now: write_many and write_many_reserved (crates/fxfs/src/object_store/data_object_handle.rs:1171, 1180; DAX writeback's path through fxfs_write_many), extents_in (2115, what the window maps from), clone_range_in_commits (2298) and zero_in_commits (2354). A dax.rs child would hold extents_in alone; write_many.rs and reflink.rs the rest. Measure the diff again with scripts/upstream-changes.py --diff before moving.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Moved, behaviour unchanged
- [ ] #2 upstream-changes --check passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
