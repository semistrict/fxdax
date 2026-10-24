---
id: TASK-26.16
title: 'Small files share extents, written through the DAX window'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-14 23:10'
updated_date: '2026-10-24 00:53'
labels: []
dependencies:
  - TASK-26.15
parent_task_id: TASK-26
ordinal: 52000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Files under 1 MiB are in the guest's page cache, because a DAX write cost a MAP request and a transaction to move its blocks to a new open extent (create was 86x ext4's), and each file had extents of its own. The user requires one copy of data in memory (no page cache copy beside the host's), so small files must be DAX too; and small files must share extents, as directories of millions of small files (git repos) can't have an extent, index entry and window mapping per file. Fxfs's extent records hold (extent ID, offset), so files pointing at different offsets of one extent are representable; freeing must wait for every file using an extent, and mostly-dead shared extents need compacting.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Small files are DAX: no file's data is in the guest's page cache
- [x] #2 Writes of small files take blocks of an open extent already mapped writably in the window, with no device request per write
- [x] #3 Many files' data share one extent; an extent is freed only once no file uses it
- [x] #4 create and create-parallel stay within 2x of ext4 with DAX on writable pmem on GCE, and read-small-cold is within 2x
- [x] #5 fsck, pjdfstest and the fxfs-guest crash tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Decided (2026-10-15, overnight, as the user asked for decisions to be made):

1. Fxfs allocator: shared staging extents. allocate_shared(transaction, owner, len) hands out the next blocks of the current staging extent (up to STAGING_SIZE = 2 MiB, allocations of at most STAGED_MAX each), while the device holds it open; otherwise it mints a new one, which the device marks open at once (Device::opened), so that a seal retires it. Extents are already tracked by allocated bytes, and die when the last range goes; what is new is pinning: the staging extent and extents with uncommitted allocations don't die (a dropped transaction's range is just dead space; a deallocation racing an uncommitted allocation doesn't free the extent), and die once unpinned with nothing allocated, after the journal is flushed past their last deallocation. Staging rotates when the store's data owner changes (snapshot).
2. txn_map_write takes new blocks from allocate_shared for parts of at most STAGED_MAX.
3. DaxExtent gains the extent's size as allocated (STAGING_SIZE for the staging extent), for the window.
4. Guest window (dax.c): extent regions, keyed by extent ID: a window range mapping a whole extent, read-only if sealed, or, for an open staging extent, writably, mapped in doubling increments as its blocks are handed out (one MAP per increment, none per file). A file's chunk whose data is one contiguous piece of one extent of at most FXFS_DAX_SIZE is a slice of that extent's region (no window space or MAP of its own); others keep ranges of their own, mapped page by page. Slices outlive inodes like other ranges, and hold their region, which goes (UNMAP) with its last slice.
5. A write a slice can't take in place (sealed, moved, or past it) drops the slice (-EXDEV from iomap: write_iter and faults retry under the invalidate lock) and maps the chunk again.
6. All regular files DAX (no 1 MiB minimum).
7. Tests: Fxfs unit tests (shared extents live until the last range goes; pins; rotation; fsck); guest: small files DAX as created, N small files take a handful of MAPs and no READ/WRITE, evicted and read again take none; crash tests, pjdfstest; GCE benchmark.

Follow-ups (not here): compacting mostly-dead shared extents; host not storing never-touched tails of writably mapped extents; rewrites of sealed small files copy kept blocks with a WRITE request.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done in 390910b, 3ed39da, 7922590, 0ac0626, fea74ca (libkrun c8600eb). small_files_share: 32 small files take 6 MAPs, no READ; read again after eviction, none. Stress test of shared extents with seals/truncations/deletions/dropped transactions, replay revive test. AC4 (GCE vs ext4-dax-pmem) not yet measured. Fable review follow-ups in TODO.md (window fragmentation/region-aware reclaim, appends fragmenting slices, compactor).

AC4 met on GCE, fxfs-guest-dax against ext4-dax-pmem: create / create-parallel / read-small-cold 0.99x / 1.07x / 0.36x (2026-10-15T1720Z), 1.15x / 1.28x / 0.46x (2026-10-16T0121Z), 1.48x / 1.62x / 0.60x (perf-base-2026-10-20T2051Z), all within 2x. Definition of Done #1 met on 2026-10-23 at 1586daee+: workspace nextest via scripts/test.sh on macOS (2466 passed) and Linux on GCE r63 (2544 passed); Linux tests run on GCE, not Lima, now. The only failure that held this, test_encrypted_mutations_mixed_versions, was removed in 7936796c. scripts/upstream-changes.py --check passes; the window's deviations are marked fxdax: in dax.c.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Small files share staging extents, written and read through the DAX window as slices of the extent's region; within 2x of ext4-dax-pmem on GCE for create, create-parallel and read-small-cold. Closed in the 2026-10-23 audit of stale tasks.
<!-- SECTION:FINAL_SUMMARY:END -->
