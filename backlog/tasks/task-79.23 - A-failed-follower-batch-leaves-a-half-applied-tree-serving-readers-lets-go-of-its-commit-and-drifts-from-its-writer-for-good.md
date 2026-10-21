---
id: TASK-79.23
title: >-
  A failed follower batch leaves a half-applied tree serving readers, lets go of
  its commit, and drifts from its writer for good
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 211000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On an apply error the barrier is released into a half-applied state, settle is skipped and Previous is let go of. The journal reader has already moved past the batch, so the rest of it is never applied. A fetch error lets go of Previous while readers still read the old trees. Only ESTALE leads to a remount.

Where: crates/follower/src/lib.rs:157, crates/follower/src/lib.rs:162, crates/follower/src/lib.rs:168, crates/fxfs/src/follower.rs:251, crates/fxfs/src/follower.rs:273, crates/fxfs/src/object_store/journal.rs:1304, guest/linux/fs/fxfs/super.c:2083.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.4, TASK-42.6, TASK-42.10. No task covers error handling of a failed follower batch: half-applied tree, Previous let go, journal reader not rewound, no remount. TASK-42.4 (Done) built apply, fetch and Previous. TASK-42.10 added the barrier and settle.
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
