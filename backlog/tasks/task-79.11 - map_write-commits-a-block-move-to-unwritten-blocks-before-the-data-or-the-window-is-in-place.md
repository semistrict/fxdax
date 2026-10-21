---
id: TASK-79.11
title: >-
  map_write commits a block move to unwritten blocks before the data or the
  window is in place
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 199000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A write fault's held page is moved, with keep=false, to a fresh unwritten block in one transaction, and its contents are restored in a later one. A seal in between makes the zeros durable, and a crash loses data the application never touched; an ENOSPC on the retry or fallback loses it without a crash. A map_extents failure after map_write leaves the remaining pieces mapping the old blocks, and the file reads zeros after reclaim or remount.

Where: guest/linux/fs/fxfs/dax.c:1600, guest/linux/fs/fxfs/dax.c:1610, guest/linux/fs/fxfs/dax.c:1625, guest/linux/fs/fxfs/dax.c:1663, guest/linux/fs/fxfs/dax.c:445, guest/linux/fs/fxfs/dax.c:1626.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.14, TASK-26.16, TASK-44.2. No task covers map_write moving a held page to unwritten blocks before its data commits, or a map_extents failure leaving stale pieces. Related: guest DAX write path (TASK-26.14, TASK-26.16) and the TLA+ of DAX seal and relocation (TASK-44.2, Done).
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
