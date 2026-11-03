---
id: TASK-150.9
title: A short extending DAX write leaves its range on a freed block
status: Done
assignee: []
created_date: '2026-11-01 16:51'
updated_date: '2026-11-03 07:51'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 524000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From the static audit, unconfirmed. dax.c:3533 omits FXFS_SET_KEEP_TAIL; crates/fxfs-posix/src/lib.rs:1662 calls zero_tail, moving the last block, while the window keeps mapping the old one writably. A later append with no seal goes to the freed block, and acknowledged bytes read as zeros once remapped. test_append_stopped_short misses it, its size being page-aligned.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test fails on the code as it stands, or shows the finding false or unreachable, which closes this as not a bug
- [x] #2 Fixed, the test red again with the fix taken out
- [x] #3 The notes name the blind spot, the test of its class, and what a search around it found
- [x] #4 Passes on GCE in the guest suites, DAX and not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-03, verified on main (dcbdd6247 plus the mutants alone). Fixed by 1d840f1c6: fxfs_vfs_resize is the one way a file's size changes, and a DAX file's tells Fxfs to keep its last block (FXFS_SET_KEEP_TAIL) rather than move it from under the window to zero it. The fix taken out (scripts/guest-mutants-gce.py resize_moves_the_last_block, KEEP_TAIL dropped) fails fxfs-guest-checks-dax at test_append_stopped_short_in_a_page, and the random script too, GCE fxdax-test-t18-ac: killed. With it, fxfs-guest-checks and fxfs-guest-checks-dax pass. Blind spot: test_append_stopped_short stopped on a page boundary, so no write stopped short within a page and left Fxfs a tail to zero. Test of the class: test_append_stopped_short_in_a_page, guest/sim's append_stopped_short_in_a_page scenario, and the mutant. Search around it: dax.c changed the size on its own in one place, now none; every size change through fxfs_vfs_resize; and who owns what lies past a file's end became TASK-153.4, which found clones, allocations and writes past the end growing a file without zeroing it.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A DAX write stopped short within a page no longer has Fxfs move the window's last block from under it: 1d840f1c6 routes every size change through fxfs_vfs_resize, which keeps the tail for DAX. Verified 2026-11-03 on main: the fix taken out (mutant resize_moves_the_last_block) fails test_append_stopped_short_in_a_page; with it, fxfs-guest-checks and fxfs-guest-checks-dax pass (GCE fxdax-test-t18-ac). DoD: workspace on Linux at ab17d2d87 (GCE r530) and macOS; upstream-changes --check passes; super.c and dax.c are fxdax's own.
<!-- SECTION:FINAL_SUMMARY:END -->
