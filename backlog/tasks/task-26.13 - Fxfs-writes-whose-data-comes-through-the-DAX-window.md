---
id: TASK-26.13
title: 'Fxfs: writes whose data comes through the DAX window'
status: Done
assignee: []
created_date: '2026-10-14 14:46'
updated_date: '2026-10-14 15:06'
labels: []
milestone: m-3
dependencies: []
parent_task_id: TASK-26
type: feature
ordinal: 49000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
For DAX writes (TASK-26.6, full DAX), Fxfs allocates a new extent for a file range and records it in the file's extent tree in a transaction, without writing data: the guest stores the data through the window (MAP_WRITE), and the next COMMIT seals the extent. Parts of the range the write doesn't cover are copied from the old data first. An inode's write stream extends the extent it has open while it stays open.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 An Fxfs API allocates and records an extent for a file range without writing it, and a test writing through it (FakeDevice) reads the data back after remount
- [x] #2 Partial blocks keep the data around them; fsck passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Device::is_open (FakeDevice: extents written since the last barrier). 2. DataObjectHandle::txn_map_write: keeps blocks in open, unshared, raw extents; moves the rest to new extents, copying the head and tail blocks' surrounding data to them; deallocates the old. 3. FakeDevice tests storing through a simulated window.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: fxfs data_object_handle::tests::test_map_write (partial head/tail blocks keep their data; read back after remount; fsck passes) and test_map_write_open_extents (open extents' blocks stay until sealed; writes past EOF see zeroes; fsck passes); storage_device fake_device test_write_once checks is_open. scripts/test.sh 1865/1865, scripts/test-linux.sh 1869/1869, upstream-changes.py --check passes. Commit 787b479. Extending an open extent past its end (rather than a new extent per write) is not done: each write's new blocks get their own extent; open blocks are rewritten in place.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fxfs can record a file range's extents for a write without writing it (txn_map_write), keeping open extents' blocks in place and copying partial blocks' surroundings to new extents. Verified with FakeDevice tests reading back after remount, with fsck, on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
