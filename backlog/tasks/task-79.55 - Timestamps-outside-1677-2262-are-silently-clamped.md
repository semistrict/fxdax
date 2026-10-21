---
id: TASK-79.55
title: Timestamps outside 1677-2262 are silently clamped
status: To Do
assignee: []
created_date: '2026-10-21 21:00'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 243000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
timespec64_to_ns clamps, but s_time_min and s_time_max are left at the TIME64 limits, so the cached time differs from the stored one until the inode is evicted.

Where: guest/linux/fs/fxfs/super.c:222, guest/linux/fs/fxfs/super.c:2360.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.19.8. No task covers s_time_min/s_time_max against u64-nanosecond storage.
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
