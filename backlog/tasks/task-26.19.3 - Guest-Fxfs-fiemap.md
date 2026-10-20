---
id: TASK-26.19.3
title: 'Guest Fxfs: fiemap'
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 15:26'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: low
type: feature
ordinal: 66000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 and btrfs implement inode_operations.fiemap, which tools such as filefrag and cp use to find holes and extents.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 FS_IOC_FIEMAP reports a file extents and holes
- [x] #2 A guest test checks a sparse file
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fiemap (and SEEK_DATA/SEEK_HOLE) landed in 67c5c64; fxfs-guest-checks' sparse check verifies FS_IOC_FIEMAP's extents and holes on a sparse file, passing with and without DAX in the full GCE run (27/27).
<!-- SECTION:FINAL_SUMMARY:END -->
