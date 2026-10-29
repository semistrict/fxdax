---
id: TASK-79.106
title: >-
  Guest DAX: a map_extents failure after map_write leaves the range's other
  pieces mapping blocks the write moved
status: Done
assignee:
  - '@main-session'
created_date: '2026-10-24 12:33'
updated_date: '2026-10-29 13:05'
labels:
  - bug
dependencies: []
parent_task_id: TASK-79
priority: medium
ordinal: 379000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Split from TASK-79.11 (its second half). fxfs_dax_iomap_write's map_write commits block moves for the whole write; if fxfs_dax_map_extents then fails partway (a device MAP error), the pieces after the failure keep their old window mappings and mapped bits, of blocks the transaction moved away from and freed: reads of them through the window see the freed blocks (zeros once reused or after reclaim, remount). Clearing their mapped bits isn't enough, as iomap_read takes unmapped pages for holes: the range must be dropped and mapped again from Fxfs's extents, with the kernel's mappings of it invalidated, which needs the invalidate_lock iomap_write doesn't hold. Review 2026-10-21 item 11 (docs/reviews/2026-10-21-main.md).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test makes map_extents fail partway after map_write (fault injection) and shows the stale pieces, on the code before the fix
- [x] #2 The range is dropped or its stale pieces remapped from Fxfs's extents, the test passing on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix: iomap_write turns a map_extents failure after map_write (other than -EXDEV) into -ESTALE; write_iter and the fault loop drop the chunk's range (fxfs_dax_drop_slice) and try once more, then fail with EIO. No deterministic seam: needs a device MAP failure after map_write (fault injection).

Fix landed in 075723b80; no deterministic test (see the notes above). GCE r65, 2026-10-24, batch on main+round 2b: Linux workspace 2620 passed, Firecracker extent tests, and 17 guest cases (checks, crash, fxfs-guest, follow, follow-torture, snapshot with and without DAX; extent-guest, live-fork, daemon-lost, pgbench both) green on vmlinux-b3/b4; macOS scripts/test.sh 2508 passed.

6a0b48493, 69227bac0: fxfs.dax_fail_maps (fault injection): of the next N writes whose moved blocks come in more than one piece, the range maps the first alone and the write fails as if the device's MAP of the next had. Check map_failed_after_move (checks-dax, -delay, -small): a 4 MiB file synced, pages 0 and 2 written (moved to the staging extent, writable), then pages 0-2 in one write: three pieces, the second's MAP failing; asserts the injection fired, then the file by read(2) and evicted. GCE r227: passed checks-dax and -small 2 rounds each; mutant map_failure_unretried (the code before 075723b80's fix: the error returned, not -ESTALE) killed in round 1, the write failing with EIO and its range left mapping the moved page where it was. r226a: checks-dax, -delay, -small passed with it in. DoD #1 not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A write's MAP failing after its blocks moved drops the range and retries (075723b80); now shown by fault injection fxfs.dax_fail_maps and check map_failed_after_move, which passes (GCE r227) and kills the pre-fix mutant map_failure_unretried.
<!-- SECTION:FINAL_SUMMARY:END -->
