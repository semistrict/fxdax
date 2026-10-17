---
id: TASK-37.4
title: >-
  Guest: the extent driver's config_changed, and Fxfs's allocator resized while
  mounted
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 16:14'
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
- [ ] #1 A mounted guest's statfs and allocations follow a grow; a shrink below what Fxfs holds is refused, shown by a guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
