---
id: TASK-153.4
title: 'To try: one owner of a file size'
status: Done
assignee: []
created_date: '2026-11-01 22:52'
updated_date: '2026-11-03 10:30'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 548000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
i_size, Fxfs size, growing and range capacity: four places, explaining TASK-92.28, 133, 147, 79.28, 150.9, 150.15 and 150.19. Buffered writes rely on the inode being ahead of Fxfs until writeback, so it needs a design first: who owns the size at each moment, and the one path every non-DAX growth takes to zero the tail.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A design says who owns the size at each moment
- [x] #2 Decided, from the design: do it or say why not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (2026-11-03). Who owns the size: in the guest the inode's i_size, always; Fxfs's size is its durable record, at most i_size but during a cut (fxfs_vfs_resize, under the inode lock and invalidate_lock), raised by writeback (write_many), DAX's records of appends (dax_update_size, map_write's size) and resize, cut by resize alone. An operation changing both changes Fxfs first where it can fail, and puts the inode back if Fxfs refused (150.15's four bugs were the inode changed first). Who owns the bytes past the end: the growth, not the cut. Every transaction raising Fxfs's size from S zeroes what the block at S holds past S unless it writes there, in one place, DataObjectHandle::grow, as upstream's grow does, where NoSpace is a legal answer. Today a clone growing its destination, and DAX's txn_update_size, bypass it, so setattr zeroes eagerly at the cut, where a shrink may not refuse, and with no free block the bytes stay (150.19, generic/130). DAX's window keeps its own zeroing of the page past i_size (fxfs_dax_zero_tail), which writes zeros to an open block before its size is recorded. Steps: 1. pin TASK-149.8's replay (150.19) as a model test, red; 2. clone's growth through grow, and every other size raise found, each red first where a test can reach it; 3. drop the eager zero_tail from setattr, and its sync-and-retry; 4. model hunts, the fxfs-posix suite, and the guest suites on GCE, DAX and not.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03, done. Fxfs: txn_grow_size, one way a write raises the size, zeroes the old last block unless the write has it; a clone and an allocation zero it, and commit, before they read the extents (zero_tail_before_growth): in one transaction they took the block the zeroing freed, an inconsistency the Linux model hunt found (a_clone_to_its_own_end_past_a_partial_block_holds). txn_zero_tail writes the block only if it isn't zeros already: written always, a writeback's growth took a block TASK-80's reservations hadn't paid for, and on a full filesystem a punch's writeback failed and an append was lost (fxfs-guest-script-full, seed 1794365991636924385, red on that version and green after, on main too). Guest: a clone growing a file zeroes its last page past the end, the cache's up to the clone and written back, a DAX file's through fxfs_dax_grow_begin, factored out of fxfs_dax_resize_begin. Kept: the cut's eager zero, best effort, for what a mapping of the last page shows past the end; the inode stays the size's owner in the guest, Fxfs's the durable record. Tests: every_growth_finds_what_a_cut_left_past_the_end_zeroed (writes in and past the block, truncate, allocations in and past it, a clone), each growth red without its part; a_truncate_with_no_block_to_zero_its_tail_in_leaves_nothing_past_the_end (TASK-149.8's replay), red before; guest check store_past_end_then_cloned. Evidence: macOS scripts/test.sh 2,012 of 2,012; fxfs and fxfs-posix 807 of 807 after the rebase; GCE fxdax-test-t18-size: Linux workspace 2,172 of 2,172 and fxfs-guest-checks, -checks-dax, -script-full, -script-full-dax passed (ee51193b8), then after rebasing on main the workspace 2,193 of 2,193 and xfstests-fxfs-guest and -dax passed (c358f029f); upstream-changes --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
One owner of what lies past a file's end: the growth, not the cut. Every non-DAX growth in Fxfs zeroes the old last block unless it writes it (txn_grow_size; clones and allocations first, committed), writing it only if it isn't zeros already; the guest's clone zeroes its last page past the end, cached or through the DAX window. A truncate with no room to zero its tail no longer leaves bytes for a later growth to bring back. Verified by new tests red without each part, the macOS suite, and on GCE the Linux workspace, the guest checks and script oracle, DAX and not, and xfstests, DAX and not.
<!-- SECTION:FINAL_SUMMARY:END -->
