---
id: TASK-79.50
title: >-
  resize_end's non-atomic bitmap_clear can bring back a writable bit after a
  seal
status: To Do
assignee: []
created_date: '2026-10-21 20:59'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 238000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
__bitmap_clear outside the gate can write back bits the lockless sealer just cleared. The next write then stores to a sealed page, and the VM stops.

Where: guest/linux/fs/fxfs/dax.c:2445, guest/linux/fs/fxfs/dax.c:1961, guest/linux/fs/fxfs/dax.c:1579.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-44.2, TASK-37.4. TASK-44.2's TLA+ seal spec does not model resize_end's bitmap_clear. TASK-37.4 added the guest resize. No existing task.
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
