---
id: TASK-79.45
title: take_over keeps the zeros VMO sized for the old window
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 20:58'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 233000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A writer that returns with a larger window maps zeros past the end of the old zeros VMO, and its VM aborts on the fault.

Where: crates/fxdax/src/extents.rs:442, crates/fxdax/src/extents.rs:692, crates/fxdax/src/extents.rs:1474, crates/fxdax/src/extents.rs:1470, crates/fxdax/src/extents.rs:1316.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.3, TASK-39.1. take_over came from TASK-42.3 and the zeros VMO (MAP_ZEROS) from TASK-39.1. No task covers the zeros VMO keeping the old window's size.
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
