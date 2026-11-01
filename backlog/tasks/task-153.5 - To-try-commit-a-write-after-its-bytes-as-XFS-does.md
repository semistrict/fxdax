---
id: TASK-153.5
title: 'To try: commit a write after its bytes, as XFS does'
status: To Do
assignee: []
created_date: '2026-11-01 22:52'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 549000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
fxfs_dax_map_write commits the move, losing the old bytes, before the copy. Explains TASK-79.11, 79.106, 119.13, 121, 133, 150.8, 150.13; a write that says EIO can still leave zeros (map_failed_writing). XFS allocates, maps, copies, then commits the extents written (xfs_reflink_end_cow). Costs a transaction spanning the gate.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A prototype
- [ ] #2 pgbench-dax measured against it, several rounds a side
- [ ] #3 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
