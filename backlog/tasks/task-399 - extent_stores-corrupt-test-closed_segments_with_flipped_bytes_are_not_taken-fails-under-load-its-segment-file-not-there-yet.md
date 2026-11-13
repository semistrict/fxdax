---
id: TASK-399
title: >-
  extent_store's corrupt test closed_segments_with_flipped_bytes_are_not_taken
  fails under load: its segment file not there yet
status: To Do
assignee: []
created_date: '2026-11-13 14:04'
labels:
  - bug
  - tests
dependencies: []
priority: low
ordinal: 784000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once on macOS (2026-11-13, TASK-395 batch, the workspace's tests running beside three simulation searches): crates/extent_store/tests/corrupt.rs:90, stored() opening segments/0000000000000007/0000000000000000 as Place::Run named it: 'doesn't exist' (MemDir, NotFound). Five runs alone after passed. Likely the test reads the segment's file before the store's framing thread (or rename into place) has written it: a race in the test's wait, or a place the store reports before its file is there. Not touched by TASK-395 (no extent_store store/segment code changed). Reproduce under load (e.g. stress-ng or several sim searches beside it) and find which.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test waits on what makes the file exist, or the store names no place whose file isn't there
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
