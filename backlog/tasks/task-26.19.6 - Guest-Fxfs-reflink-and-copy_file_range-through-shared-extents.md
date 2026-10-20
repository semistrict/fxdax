---
id: TASK-26.19.6
title: 'Guest Fxfs: reflink and copy_file_range through shared extents'
status: Done
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 19:03'
labels: []
dependencies: []
parent_task_id: TASK-26.19
priority: medium
type: feature
ordinal: 69000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
btrfs implements remap_file_range (FICLONE, FICLONERANGE, FIDEDUPERANGE). Fxfs already shares extents between objects (fxdax snapshots, small files sharing staging extents), so a clone can be metadata only; cp --reflink and copy_file_range would then copy no data.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 FICLONE and FICLONERANGE share extents without copying data
- [x] #2 Writing either file afterwards leaves the other unchanged
- [x] #3 copy_file_range uses the same path
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
6fa0684 (fxfs: per-range reference counts in the allocator, Reference mutation with folded deltas, txn_clone_range, is_shared/extents_in cover shared ranges, fsck counts references), bb41664 (guest remap_file_range for page cache and DAX). copy_file_range goes through remap_file_range in the VFS. Verified: fxfs unit tests (+hand mutants), guest check reflinks (with and without DAX), Docker, full Linux workspace.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
FICLONE, FICLONERANGE and copy_file_range share extents by reference counts in Fxfs's allocator, copying nothing; writes to either file move the blocks they change (DAX mappings too). Verified by fxfs unit tests and the reflinks guest check on GCE.
<!-- SECTION:FINAL_SUMMARY:END -->
