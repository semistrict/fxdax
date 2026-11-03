---
id: TASK-160.24
title: 'Fxfs: fxdax-only code in child modules of upstream files'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
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
data_object_handle.rs is +2,596/-141 against upstream, 1,637 lines of it pure additions (write_many, extents_in, txn_map_write, clones, zero_in_commits); volume.rs +1,197, 856 pure (snapshots, bases). Proposal: data_object_handle/{dax,write_many,reflink}.rs and volume/{snapshot,base}.rs, impl blocks; the upstream diff of those files shrinks by about 2,500 lines. After TASK-153.5 and 153.12.
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
