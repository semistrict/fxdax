---
id: TASK-79.46
title: Open-extent READ and WRITE check open and do their I/O under different locks
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
ordinal: 234000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A READ racing a commit returns zeros as a success. A WRITE racing the first MAP of the same extent is overwritten by the VMO at commit.

Where: crates/fxdax/src/extents.rs:970, crates/fxdax/src/extents.rs:978, crates/fxdax/src/extents.rs:1085, crates/fxdax/src/extents.rs:1095, crates/fxdax/src/extents.rs:1502, crates/fxdax/src/extents.rs:1234.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-59, TASK-32, TASK-26.12. No task covers the open-extent READ/WRITE lock race. TASK-59 (To Do, cause unknown: postgres 'unexpected end of data' in a parallel index build) shows a short or wrong read and could be a symptom, but it is not this defect.
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
