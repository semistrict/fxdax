---
id: TASK-80
title: >-
  Guest Fxfs: buffered writes to files not on DAX return ENOSPC when the
  filesystem is full
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-22 12:37'
updated_date: '2026-10-27 19:21'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fxfs (crates/fxfs): DirtyReservation, room for a store's files' dirty data by file: each block and its share of a transaction's metadata space (META/WRITE_MANY_WRITES), and a spare (a transaction's metadata) per file with dirty data; reserve (NoSpace), reserve_what_fits (owed), release, take/written for a writeback. DataObjectHandle::write_many_reserved pays blocks from what was taken, holding metadata beyond the blocks still to write, topped up from the allocator, else borrowing it.
2. fxfs-linux: Fxfs::dirty, FFI fxfs_reserve_dirty / fxfs_reserve_dirty_what_fits / fxfs_release_dirty; fxfs_write_many takes the bytes of reserved pages it writes.
3. Guest kernel (super.c): folio state on every dirtied folio, with dirty and reserved bitmaps; write_begin reserves the pages it dirties (ENOSPC after sync_filesystem to make room), write_end releases what a short copy didn't dirty; page_mkwrite reserves (SIGBUS); zero_past_end reserves; writeback passes each run's reserved bytes; invalidate releases.
4. Tests: Fxfs unit tests (fill to NoSpace, write back a block at a time and 128 at a time, filesystem kept full; release; owed); guest check 'enospc' registered without DAX, checking that what fitted reads back after writeback.
5. GCE: fxfs-guest-checks[-dax] enospc, then the default guest suites and workspace.
<!-- SECTION:PLAN:END -->
