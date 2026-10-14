---
id: TASK-26.14
title: 'Guest Fxfs: file data through the DAX window (fsdax)'
status: To Do
assignee: []
created_date: '2026-10-14 14:46'
labels: []
milestone: m-3
dependencies:
  - TASK-26.12
  - TASK-26.13
parent_task_id: TASK-26
type: feature
ordinal: 50000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2's data path, with the user's choice of full DAX (2026-10-14): regular files are DAX inodes when the extent device has a DAX window with DAX_WRITE, as virtio-fs's DAX does. Reads and mmap map extents' ranges from the window (iomap over MAP), so file pages are not in the guest's page cache; writes allocate extents through Fxfs (map-only writes) and store through writable mappings. The driver provides a dax_device over the window. Without a window, files stay in the page cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 With a DAX window, files report STATX_ATTR_DAX, and reads, writes, mmap (shared and writable) and truncation pass fxfs-guest's and virtiofs-fxdax's checks
- [ ] #2 Reading a file maps it instead of issuing READ requests (the device's request counts)
- [ ] #3 pjdfstest and the crash test pass with DAX
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
