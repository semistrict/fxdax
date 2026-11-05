---
id: TASK-160.2
title: 'Guest DAX: resize, punch and clone driven from one module the sim builds'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 03:40'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. data.c: fxfs_data_resize, fxfs_data_fallocate (its punch) and fxfs_data_remap_file_range, moved from super.c, over struct fxfs_data_ops (vfs.h), an inode's, set with i_fop and a_ops: the page cache's in super.c, the window's in dax.c (resize_begin/end, keeps_tail, punch_begin, zero, punch_drop, remap_begin, remapped). The punch's partial-page split is written once, in data.c.
2. guest/sim builds data.c: sim_truncate calls fxfs_data_resize, sim_punch the file's fallocate, sim_clone its remap_file_range; kernel/vfs.c fakes generic_remap_file_range_prep and the two-inode locks; the model takes write_inode, says a clone's attributes and attr_of_open. fsync stays super.c's and the sim's: its data part is fxfs_dax_write_back, the rest VFS metadata.
3. Behaviour: the model fails a growth (fxfs_write_inode) as fxfs_sim_fail asks; interleave checks that a failed growth leaves the size; mutant task-160.2-resize-no-rollback.
4. mutants.sh --check, zloop.sh -s 3000, every mutant; guest kernel build; GCE DAX suites.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: waits for TASK-164 (read-only DAX), which changes what this covers; re-scope it then.
<!-- SECTION:NOTES:END -->
