---
id: TASK-79.106
title: >-
  Guest DAX: a map_extents failure after map_write leaves the range's other
  pieces mapping blocks the write moved
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-24 12:33'
updated_date: '2026-10-24 12:48'
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
- [ ] #1 A test makes map_extents fail partway after map_write (fault injection) and shows the stale pieces, on the code before the fix
- [ ] #2 The range is dropped or its stale pieces remapped from Fxfs's extents, the test passing on GCE
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
<!-- SECTION:NOTES:END -->
