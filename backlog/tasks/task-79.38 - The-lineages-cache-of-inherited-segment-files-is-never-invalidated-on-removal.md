---
id: TASK-79.38
title: The lineage's cache of inherited segment files is never invalidated on removal
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
ordinal: 226000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Removed segments stay cached, so up to 64 unlinked segments keep their disk space. A reused branch ID then reads the old segment file's bytes, with valid CRCs, or panics indexing past its parts.

Where: crates/extent_store/src/lineage.rs:99, crates/extent_store/src/lineage.rs:354, crates/extent_store/src/lineage.rs:645, crates/extent_store/src/lineage.rs:480, crates/extent_store/src/framed.rs:141, crates/extent_store/src/store.rs:1272.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-34, TASK-39.4, TASK-28.2. No task covers the lineage's inherited-segment cache never being evicted on removal, or Source::locate's bounds. TASK-34 added shared segment files (64 at most). TASK-39.4 covers deleting bases while forks live.
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
