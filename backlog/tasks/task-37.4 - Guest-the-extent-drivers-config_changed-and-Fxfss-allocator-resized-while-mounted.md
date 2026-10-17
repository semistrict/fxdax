---
id: TASK-37.4
title: >-
  Guest: the extent driver's config_changed, and Fxfs's allocator resized while
  mounted
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
updated_date: '2026-10-17 16:56'
labels: []
dependencies: []
parent_task_id: TASK-37
type: feature
ordinal: 109000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
virtio_extent's driver gains .config_changed, which re-reads capacity and calls a callback fs/fxfs registers (as fxfs_device_set_sealer does), from a workqueue; Rust fxfs_resize sets ExtentDevice's capacity (atomic) and Fxfs's Allocator::set_device_size (a deviation from upstream, documented), which refuses to shrink below what is allocated and reserved; the guest answers a shrink with what it took.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A mounted guest's statfs and allocations follow a grow; a shrink below what Fxfs holds is refused, shown by a guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Guest driver config_changed and virtio_extent_capacity; fs/fxfs work item calls Rust fxfs_resize, which grows ExtentDevice capacity and Fxfs's allocator (Allocator::set_device_size, an fxdax deviation), takes a shrink only with 16 MiB headroom over what Fxfs holds, and answers RESIZED. Verified by fxfs test_device_size_changes and fxfs-guest-resize in Lima.
<!-- SECTION:FINAL_SUMMARY:END -->
