---
id: TASK-80
title: >-
  Guest Fxfs: buffered writes to files not on DAX return ENOSPC when the
  filesystem is full
status: To Do
assignee: []
created_date: '2026-10-22 12:37'
labels: []
dependencies: []
priority: high
ordinal: 254000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A buffered write (fxfs_vfs_write_begin/write_end in guest/linux/fs/fxfs/super.c) only dirties the page cache: nothing reserves space for it. On a full filesystem, write(2) keeps succeeding; writeback (fxfs_vfs_writepages) then fails, mapping_set_error records it, and the folios are dropped, so the data is lost unless the writer fsyncs and checks. Found by the 'enospc' guest check (TASK-60): on a 256 MiB store without DAX, a file written a MiB at a time never got ENOSPC and the check timed out after 180 s. DAX files are fine: their writes allocate synchronously.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 write(2) of a file not on DAX returns ENOSPC, at the write, once the filesystem has no room for what it dirties
- [ ] #2 The 'enospc' guest check passes in fxfs-guest-checks (without DAX)
- [ ] #3 Writeback never drops dirty data because the filesystem filled after the write
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
