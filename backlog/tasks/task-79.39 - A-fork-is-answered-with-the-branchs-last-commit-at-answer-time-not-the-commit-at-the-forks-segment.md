---
id: TASK-79.39
title: >-
  A fork is answered with the branch's last commit at answer time, not the
  commit at the fork's segment
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 227000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
If a commit lands in the same gather as the fork, the child's marker names a commit its branch does not have, and every open of the child fails with 'moved on elsewhere'. It needs the store to change during the fork, against the documented precondition.

Where: crates/fxdax/src/remote.rs:690, crates/fxdax/src/remote.rs:692, crates/fxdax/src/extents.rs:861, crates/fxdax/src/remote.rs:716.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-28.7, TASK-39.3. TASK-28.7 notes 'the child's marker names the inherited last commit', the design point here. No task tracks the race where the fork is answered with a later commit.
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
