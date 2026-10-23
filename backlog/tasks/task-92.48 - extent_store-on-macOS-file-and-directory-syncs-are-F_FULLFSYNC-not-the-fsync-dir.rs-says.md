---
id: TASK-92.48
title: >-
  extent_store on macOS: file and directory syncs are F_FULLFSYNC, not the fsync
  dir.rs says
status: To Do
assignee: []
created_date: '2026-10-23 06:12'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: task
ordinal: 339000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.42 (round 2a). crates/extent_store/src/dir.rs:106-107 says a macOS file sync is a plain fsync, leaving the drive cache to barrier's single F_FULLFSYNC, but sync_cached, sync and sync_dir call sync_all, which Rust's std implements as F_FULLFSYNC on Apple (as it does sync_data), so every file and directory sync flushes the drive cache. macOS is a development host, but every macOS store-bench figure (TASK-92.42's included) measures drive-cache flushes. Using libc::fsync there has a durability trade-off to state.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 dir.rs's macOS syncs do what its doc says, or the doc says what they do and why
- [ ] #2 store-bench's burst figures on macOS before and after are in the task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
