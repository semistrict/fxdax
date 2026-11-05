---
id: TASK-149.30
title: >-
  Neither guest/sim nor a guest suite finds a resize that moves a DAX file's
  last block from under the window
status: Done
assignee:
  - '@claude'
created_date: '2026-11-05 15:01'
updated_date: '2026-11-05 22:31'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-160.32 ran every hand mutant against its killer. task-150.9-resize-moves-the-last-block (guest/mutants), data.c's resize without FXFS_SET_KEEP_TAIL, so that Fxfs zeroes a DAX file's last block past the end by moving it from under the window, is killed by nothing since copying on write: fxfs-guest-checks-dax's append_stopped_short_in_a_page killed it on 2026-11-03 (GCE fxdax-test-t18-ac), and on 2026-11-05 passed with it in, alone in 3 rounds and with the suite's every check (GCE fxdax-test-r625); guest/sim fails no one-run test, scenario, or 300 seeds of interleave and failures with it in, though its model of Fxfs moves the block so (zero_tail, fxfs.c). The bug is real (TASK-150.9), so this is a blind spot of both: a window page reading the old block until it is freed, a model freeing it later than Fxfs, or no check reading the block after the flush that frees it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The reason no simulator run fails with task-150.9-resize-moves-the-last-block in is recorded
- [ ] #2 A simulator run fails with it in, and its header names that run as its killer, Kernel: sim
- [ ] #3 The run passes without it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Decide whether task-150.9-resize-moves-the-last-block can still be wrong copying on write: follow a DAX cut through data.c (fxfs_data_resize) and dax.c (fxfs_dax_resize_begin, zero_part, resize_end), the model's move_tail, and Fxfs's txn_zero_tail.
2. Equivalent: record why in the mutant's header and close; a gap: a guest/sim scenario that kills it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Verdict: equivalent copying on write, so no killer: criteria 2 and 3 are left unchecked because no run can fail with the mutant in.

Why: FXFS_SET_KEEP_TAIL affects Fxfs only on a cut (fxfs-posix lib.rs:1696, 'if cut && ...'; the model's fxfs_write_inode likewise), and a growth zeroes the old end's block anyway after fxfs_dax_grow_begin dropped its range. For a cut into a block, fxfs_dax_resize_begin first writes back what is dirty, then zeroes the last page past the new end through the window (fxfs_dax_zero_part). That iomap makes the page private, a copy of the old block, and dirty, before data.c's fxfs_vfs_write_attr has Fxfs move the block. Nothing reads the moved block's old place through the window after that. A private page goes only by a MAP or UNMAP after its writeback: a drop writes dirty pages back first, resize_end maps zeros only past the end, and a rebind maps Fxfs's sealed extents as they are then. A writeback and rebind can come between the zeroing and resize_begin's wb_lock. Then the page maps a block holding zeros past the end already, which Fxfs's txn_zero_tail returns from without moving (crates/fxfs/src/object_store/data_object_handle/tail.rs:74; the model's move_tail likewise). So the mutant costs one block written per cut. A crash in between leaves the file's bytes intact either way. This fits GCE r625 (fxfs-guest-checks-dax passed with the mutant in, 3 rounds) and 300 sim seeds.

Written in the mutant's header (Killer: none, with the reason). Decision recorded in OPEN_QUESTIONS.md # Decisions.

Verified: guest/sim mutants.sh --check (34 checked, 0 broken); zloop.sh -s 3000 passed; the full sim mutant run (fate none, as its header says; every other sim mutant killed); scripts/guest-mutants-gce.py --check (38, 0 broken); FXDAX_TEST_PARTS=scripts scripts/test.sh passed; scripts/upstream-changes.py --check passed. DoD 1 not run: no Rust changed, and GCE is unavailable (gcloud sign-in).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
task-150.9-resize-moves-the-last-block is equivalent copying on write. A DAX cut makes its last page private, by zeroing it past the end through the window, before Fxfs moves the block, and Fxfs never moves a block that already holds zeros past the end. So the window never reads the moved block's old place. The mutant's header now says so (Killer: none); no killer was possible. Verified that every sim gate still passes (mutants.sh --check, zloop -s 3000, the full mutant run, guest-mutants-gce.py --check, scripts tests).
<!-- SECTION:FINAL_SUMMARY:END -->
