---
id: TASK-114
title: >-
  Guest DAX: relocated set by mmap faults blocks pruning forever for files later
  written with write(2) (suspected)
status: To Do
assignee: []
created_date: '2026-10-28 12:45'
labels:
  - bug
dependencies: []
ordinal: 431000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28): fxfs_dax_prune returns unless relocated is empty; fxfs_dax_evacuee_place clears relocated only if the inode has no DAX entries for those pages; fxfs_dax_cow clears them only on faults. A file once mmapped, then written with write(2), may keep relocated set forever: its regions are never pruned (pool fills -> window_regions_full -> slow write-path range MAPs) and partial-page writes take IOMAP_F_SHARED copy-around. Unverified.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Confirmed or refuted with a guest test (mmap, unmap, then write(2) + fsync loop; regions pruned)
- [ ] #2 If confirmed, regions of such files are pruned
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
