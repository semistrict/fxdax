---
id: TASK-80
title: >-
  Guest Fxfs: buffered writes to files not on DAX return ENOSPC when the
  filesystem is full
status: Done
assignee:
  - '@claude'
created_date: '2026-10-22 12:37'
updated_date: '2026-10-27 20:29'
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
- [x] #1 write(2) of a file not on DAX returns ENOSPC, at the write, once the filesystem has no room for what it dirties
- [x] #2 The 'enospc' guest check passes in fxfs-guest-checks (without DAX)
- [x] #3 Writeback never drops dirty data because the filesystem filled after the write
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fxfs (crates/fxfs): DirtyReservation, room for a store's files' dirty data by file: each block and its share of a transaction's metadata space (META/WRITE_MANY_WRITES), and a spare (a transaction's metadata) per file with dirty data; reserve (NoSpace), reserve_what_fits (owed), release, take/written for a writeback. DataObjectHandle::write_many_reserved pays blocks from what was taken, holding metadata beyond the blocks still to write, topped up from the allocator, else borrowing it.
2. fxfs-linux: Fxfs::dirty, FFI fxfs_reserve_dirty / fxfs_reserve_dirty_what_fits / fxfs_release_dirty; fxfs_write_many takes the bytes of reserved pages it writes.
3. Guest kernel (super.c): folio state on every dirtied folio, with dirty and reserved bitmaps; write_begin reserves the pages it dirties (ENOSPC after sync_filesystem to make room), write_end releases what a short copy didn't dirty; page_mkwrite reserves (SIGBUS); zero_past_end reserves; writeback passes each run's reserved bytes; invalidate releases.
4. Tests: Fxfs unit tests (fill to NoSpace, write back a block at a time and 128 at a time, filesystem kept full; release; owed); guest check 'enospc' registered without DAX, checking that what fitted reads back after writeback.
5. GCE: fxfs-guest-checks[-dax] enospc, then the default guest suites and workspace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fxfs: DirtyReservation (object_store/dirty_reservation.rs) reserves, by file, each dirty block, its share of a transaction's metadata space (TRANSACTION_METADATA_MAX_AMOUNT/WRITE_MANY_WRITES) and a spare per file; DataObjectHandle::write_many_reserved pays blocks from what a writeback took, holding metadata only beyond the blocks still to write (topped up from the allocator, else borrowed). Guest: write_begin/page_mkwrite/zero_past_end reserve the pages they dirty (folio state now on every dirtied folio, with dirty and reserved bitmaps), syncing once to make room, else ENOSPC/SIGBUS; write_end releases what a short copy didn't dirty; writeback passes each run's reserved bytes (fxfs_write_many's new argument); invalidate releases; sync waits for the graveyard so removed files' room is free. Evidence: Fxfs tests test_dirty_data_reserved_is_written_however_full and _a_block_at_a_time_ fail with write_many ignoring the reservation, pass with it; guest check enospc without DAX on main's kernel: 1 GiB written into 256 MiB without ENOSPC; with the fix 3 rounds pass in fxfs-guest-checks and -dax (ENOSPC at the write, sync_all OK, everything that fitted read back from Fxfs after its pages were dropped). GCE: default guest suites all OK (pgbench no-DAX 1644-1684 tps, DAX 1108->902); Linux workspace 2658 passed; macOS 2530 passed; upstream-changes.py --check OK.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Buffered writes (files not on DAX) now reserve room in Fxfs as they dirty pages, and fail with ENOSPC at the write when the filesystem hasn't it (after one sync to make room); writeback pays for its writes from that reservation, so a filesystem filled later never makes it drop data. Fxfs gains DirtyReservation and write_many_reserved; the guest kernel tracks reserved pages per folio and passes them through writeback; sync waits for the graveyard. Verified: Fxfs unit tests (killed by a mutant ignoring the reservation), the enospc guest check without DAX (fails on main: no ENOSPC for 1 GiB into 256 MiB; passes 3 rounds with the fix, data read back), default guest suites, workspace on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
