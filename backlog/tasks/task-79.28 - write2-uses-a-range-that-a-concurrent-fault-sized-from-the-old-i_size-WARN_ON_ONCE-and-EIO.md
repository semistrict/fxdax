---
id: TASK-79.28
title: >-
  write(2) uses a range that a concurrent fault sized from the old i_size:
  WARN_ON_ONCE and -EIO
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-10-24 14:08'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 216000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
write_iter drops undersized ranges, then releases the invalidate_lock before writing. A fault can recreate the range from the old i_size, and iomap_write uses it without checking that it covers the write, so an append concurrent with mmap reads fails with EIO and a WARN.

Where: guest/linux/fs/fxfs/dax.c:1791, guest/linux/fs/fxfs/dax.c:1539, guest/linux/fs/fxfs/dax.c:1559, guest/linux/fs/fxfs/dax.c:1293.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.14, TASK-26.18.8, TASK-44.2. No task covers write_iter using a range a concurrent fault rebuilt from the old i_size. Related: DAX file data (TASK-26.14) and PMD read faults (TASK-26.18.8, Done, the latest DAX fault change).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24 attempt, reverted with TASK-79.12's change: write_iter fitted the ranges inside the invalidate_lock hold across the copy; that hold hung fxfs-guest-crash-dax. A fix needs the fit and the write under one hold without that hang. The check append_while_mapped passes on old and new kernels (the race is narrow).
<!-- SECTION:NOTES:END -->
