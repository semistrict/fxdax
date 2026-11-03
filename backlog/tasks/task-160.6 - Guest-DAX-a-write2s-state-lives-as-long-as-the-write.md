---
id: TASK-160.6
title: 'Guest DAX: a write(2)''s state lives as long as the write'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: low
type: task
ordinal: 577000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
through, no_slices and in_hand live on fxfs_dax_file and are set and reset by hand in write_iter and iomap_end. Proposal: struct fxfs_dax_writing on write_iter's stack, reached through file->writing under the inode lock. growing stays on the file (faults of other tasks read it, TASK-147); append_* spans writes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No per-write field of fxfs_dax_file is reset by hand
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
