---
id: TASK-160.2
title: 'Guest DAX: resize, punch and clone driven from one module the sim builds'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
updated_date: '2026-11-04 07:34'
labels:
  - structure
dependencies:
  - TASK-164
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 573000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
super.c isn't built in guest/sim; its resize, punch, clone and fsync are rewritten in guest/sim/kernel/mount.c and have drifted (no size rollback, resize_end's argument, no blocks rounding or attr_of_open recovery in sim_clone). The partial-page split of a punch is written twice (super.c and dax.c fxfs_dax_punch_begin). Explains TASK-150.9, 153.4, 79.53, 2d8e33fef. Proposal: data.c with resize, punch, remap and fsync over a seam with two adapters (DAX, page cache); super.c keeps the VFS glue; the sim builds data.c and drops its copies.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The sim builds the real resize, punch and clone
- [ ] #2 A mutant removing the resize rollback is found by the sim
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: waits for TASK-164 (read-only DAX), which changes what this covers; re-scope it then.
<!-- SECTION:NOTES:END -->
