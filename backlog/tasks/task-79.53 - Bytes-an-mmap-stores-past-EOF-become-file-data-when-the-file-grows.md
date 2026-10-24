---
id: TASK-79.53
title: Bytes an mmap stores past EOF become file data when the file grows
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-24 15:11'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 241000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Writeback never zeroes the EOF folio's tail, and growth does not zero it either, so mmap stores past EOF appear as file data after the file is extended.

Where: guest/linux/fs/fxfs/super.c:1409, guest/linux/fs/fxfs/super.c:1354, guest/linux/fs/fxfs/super.c:712, guest/linux/fs/fxfs/super.c:2364.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.11, TASK-26.6. No task covers the guest Fxfs not zeroing the EOF folio tail. TASK-6 and TASK-7 cover past-EOF behaviour only in the old userspace virtio-fs DAX path.
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
Fix in 075723b80 (the EOF folio's tail zeroed on growth); store_past_end passes on GCE. Not yet seen failing on the kernel before: that kernel oopses at short_copy, earlier in the same check run.
<!-- SECTION:NOTES:END -->
