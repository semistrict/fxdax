---
id: TASK-79.66
title: >-
  extent_store tests: MemDir's create of a path that exists took the entry for
  an inode it never made
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 317000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.5 (round 1 of the review fixes). MemDir, the in-memory directory the store's tests run on, set a path's entry to a new inode before failing a create_new of a path that existed with AlreadyExists. The next file created took that inode, so the file at the path read the next file's data. Real directories leave the entry alone; TASK-79.5's re-creation of a raw file left at its path goes through exactly that failure, so the model must too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A create_new of an existing path in MemDir fails AlreadyExists and leaves the path's file and data as they were, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
When MemDir created a path that already had an entry, it pointed the entry at a new inode before failing with AlreadyExists. The next file created took that inode, so the existing path read the new file's data. Fs::change now refuses an add over an existing entry before changing anything. mem::tests::creating_where_a_file_is_leaves_it failed before the fix ('left: ([98], [98]) right: ([97], [98])') and passes on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
