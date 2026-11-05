---
id: TASK-160.2
title: 'Guest DAX: resize, punch and clone driven from one module the sim builds'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 10:18'
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
- [x] #1 The sim builds the real resize, punch and clone
- [x] #2 A mutant removing the resize rollback is found by the sim
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
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

2026-11-04/05 (task-160-4: 22c67828b refactor, 3b45ee55c behaviour). Design: guest/linux/fs/fxfs/data.c holds fxfs_data_resize (was super.c's fxfs_vfs_resize, with the rollback), fxfs_data_fallocate and its punch (was fxfs_vfs_fallocate/fxfs_vfs_punch; the partial-page split now written once) and fxfs_data_remap_file_range (was fxfs_vfs_remap_file_range: blocks rounding, attr_of_open recovery, i_blocks). Each drives struct fxfs_data_ops (vfs.h), which the inode points to as it does i_fop and a_ops: the page cache's (super.c: resize_begin zero_past_end; punch_begin write back and invalidate; zero fxfs_vfs_write_zeros; remap_begin zero past the end to the clone; remapped invalidate) set in alloc_inode, the window's (dax.c: resize_begin/end, keeps_tail, punch_begin break_layouts, zero fxfs_dax_zero_part, punch_drop fxfs_dax_drop, remap_begin) set by fxfs_dax_set_inode. fxfs_dax_resize_begin/end, punch and remap are static; vfs.h exports fxfs_vfs_write_attr. guest/sim builds data.c (Makefile, lib.sh, mutants): sim_truncate is fxfs_data_resize, sim_punch the file's fallocate, sim_clone its remap_file_range; kernel/vfs.c fakes generic_remap_file_range_prep (writes both ranges back) and the two-inode locks; mount.c keeps fsync and a write_attr of the size alone; the model takes write_inode (was fxfs_setattr), fills a clone's attr, has attr_of_open. fsync stays out (OPEN_QUESTIONS decision): its data half is one call.
AC #2: the model fails a growth in write_inode as fxfs_sim_fail asks (a cut never, as Fxfs borrows); interleave checks a failed growth leaves the size. Mutant task-160.2-resize-no-rollback: caught at seed 1 (seed 8 with -J 2); without the new check, at seed 233 by check_extents' size (the blind spot: no oracle at the failure, only the end's).
Verified: kernel builds (arm64, Docker debian:trixie, scripts/build-guest-kernel.sh, after data.c's include order fix); guest/sim check.sh -s 400 0 failed; zloop.sh -s 3000 passed twice (487 s, 456 s); mutants.sh --check 29/0; every mutant caught but the five known (patch-0012, task-153.1, task-164.4-truncate-unwritten, task-79.28-range-too-small, task-44.19), none/2000 each. GCE r570: fxfs-guest-dax passed; the other three killed by Firecracker's seccomp (pread64 in make_private, host side, TASK-169).

GCE r571 (FXDAX_FIRECRACKER_SECCOMP=log, us-west1-b, us-west1-a full): fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax and xfstests-fxfs-guest-dax all passed; the log filter logged pread64 alone (TASK-169).

GCE r584 on main c919610e7: Linux workspace 2249/2249; CI 41994971122; guest suites since: r583, r585 (jailed) pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
data.c holds resize (with its rollback), fallocate and punch (the partial-page split once) and remap over fxfs_data_ops, one per inode (page cache's, the window's); guest/sim builds it in place of its drifted copies; task-160.2-resize-no-rollback caught by the sim. GCE r584 on main c919610e7: Linux workspace 2249/2249; CI 41994971122; guest suites since: r583, r585 (jailed) pass.
<!-- SECTION:FINAL_SUMMARY:END -->
