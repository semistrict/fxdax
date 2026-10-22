---
id: TASK-79.57
title: >-
  The legacy virtio-fs Filesystem's listing, rename and unlink deviate from
  POSIX
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-22 21:07'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 245000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Concurrent opendir calls share one seek cursor, and their listings lose or repeat entries. NOREPLACE is a separate check and rename, so a concurrent create is replaced. unlink removes empty directories.

Where: crates/fxdax/src/filesystem.rs:820, crates/fxdax/src/connection.rs:154, crates/vfs/src/directory/connection.rs:42, crates/fxdax/src/filesystem.rs:705, crates/fxdax/src/filesystem.rs:672.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-24, TASK-5, TASK-2. TASK-24 (To Do) lists other legacy virtio-fs robustness bugs: DESTROY, max_pages, descriptors, leaked futures. It does not cover the shared opendir cursor, the NOREPLACE race, or unlink removing empty directories.
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
