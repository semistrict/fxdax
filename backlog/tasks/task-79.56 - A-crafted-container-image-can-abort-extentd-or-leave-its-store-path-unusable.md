---
id: TASK-79.56
title: 'A crafted container image can abort extentd, or leave its store path unusable'
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 06:48'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 244000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Recursive whiteout removal over deep paths overflows the stack and aborts the daemon. An out-of-range tar mtime panics the import, and the half-made store is left behind, so later imports get EEXIST.

Where: crates/fxdax/src/import.rs:457, crates/fxdax/src/import.rs:466, crates/fxdax/src/import.rs:493, crates/fxdax/src/import.rs:66, crates/fxdax/src/extentd/stores.rs:377.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-39.5. TASK-39.5 (Done) built the import. No task covers recursive whiteout removal, out-of-range tar mtimes, or cleanup on unwind.
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
