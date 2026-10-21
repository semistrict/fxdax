---
id: TASK-79.6
title: >-
  The shared extent cache is keyed by guest extent IDs, which the host never
  enforces as write-once
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
ordinal: 194000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cache entries keyed by (arena, ExtentId) are shared across branches, and the host lets a branch FREE and then rewrite or writably MAP one of its own IDs. On commit, seal_mapped moves the new pages into another store's entry for the old extent, so a writer guest can inject data into a clone or fork that still holds that extent.

Where: crates/fxdax/src/extents.rs:1690, crates/fxdax/src/extents.rs:1288, crates/fxdax/src/extents.rs:1322, crates/extent_store/src/store.rs:1385.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity high): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-26.4, TASK-26.12, TASK-42.2, TASK-43.2. Not tracked. The extent cache keyed by extent ID (TASK-26.4, TASK-26.12) and by arena (TASK-43.2) both rest on 'IDs are never reused' (TASK-42.2), which the host never enforces for a branch's own IDs.
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
