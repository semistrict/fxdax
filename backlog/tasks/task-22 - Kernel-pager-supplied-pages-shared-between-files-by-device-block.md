---
id: TASK-22
title: 'Kernel: pager-supplied pages shared between files by device block'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 14:05'
updated_date: '2026-10-10 16:11'
labels: []
dependencies:
  - TASK-21
type: feature
ordinal: 22000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Goal: volumes that share extents (TASK-21 snapshots) also share their cached pages in memory, so many guests cloned from one filesystem cost the memory of one for what they haven't changed.

Today each file's pages live in its own pager-backed VMO, filled by Fxfs's pager with supply_pages, which moves pages into the VMO (crates/zircon/src/vm, object/pager_proxy.rs; crates/fxfs-platform/src/fuchsia/pager.rs). Zircon's only page sharing is copy-on-write VMO children, which are not ported, and they don't fit: a child of a file's VMO isn't pager-backed, so its dirty pages couldn't go through the pager's dirty tracking and writeback, and a child of a changing parent sees the parent's later writes. Linux doesn't share page cache between reflinked files either.

Design to work out in the plan: a primitive by which the pager supplies a page as a shared reference to a page cached by device block (e.g. a cache keyed by (device, block) for extents the allocator marks shared); every VMO mapping it maps the same physical page read-only; a write fault raises the usual DIRTY request, and the pager's acknowledgment forks a private copy into that VMO before it is dirtied, so writeback is unchanged; the page queues, accessed-bit harvesting, eviction (a shared page is evicted only when no VMO holds it, or is unshared per VMO) and memory accounting handle shared pages. Fxfs's pager uses it for reads of shared extents. This extends the Zircon port beyond upstream, so the deviations are documented in the kernel sources.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The kernel can supply one physical page to several pager-backed VMOs at once, read-only until written
- [x] #2 A write to a shared page raises a DIRTY request and the write lands in a private copy; the other VMOs keep the original, shown by tests in zircon-core-tests
- [x] #3 Page queues, the scanner, the evictor and memory accounting handle shared pages: a shared page counts once and is freed when no VMO holds it
- [x] #4 Fxfs's pager supplies pages of shared extents through it
- [x] #5 Reading the same file in a volume and its snapshot uses one page per block, measured by a test
- [x] #6 DAX mappings of shared pages work, and a guest write through DAX forks the page
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
- [x] #4 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #5 libkrun guest tests pass on macOS and Linux
- [x] #6 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design: a cache VMO per frozen owner lends its pages; files' VMOs borrow them.
- Kernel (VmCowPages::lend_pages): a borrowed page is a page in a borrower's page list whose backlink is its lender. It stays in the lender's list, page queue and accounting. Lent pages are always Clean and unpinned: the lender takes back (revoke_loans) every loan of a page before dirtying, pinning, evicting or removing it, taking each borrower's lock under its own. A borrower copies a borrowed page before writing, dirtying or pinning it (replace_borrowed_page_locked: in require_owned_page, which writes and commits go through, and dirty_pages, for DIRTY acks), skips them in its own eviction and hints, and gives them back instead of freeing them on every removal path (BatchPqRemove, remove_page_locked). An object only lends or borrows, fixed by its first loan, so locks only nest lender then borrower. Loans are recorded in the lender under a leaf lock.
- Syscall zx_pager_supply_shared_pages(pager, pager_vmo, offset, len, src_vmo, src_offset): like supply_pages, populating only absent content; all-or-nothing NOT_FOUND unless every source page is present, clean and unpinned. zx wrapper Pager::supply_shared_pages; zx_info_vmo counts borrowed pages as populated, not committed.
- Fxfs: Allocator::frozen_ranges and DataObjectHandle::frozen_pages find a file's pages whose content is data a frozen owner holds (unencrypted, one fully written extent, one owner). Frozen data never changes, so no invalidation is needed.
- Platform: SharedBlocks (per filesystem, in VolumesDirectory) keeps a cache VMO per frozen owner, indexed by device block, never faulted. Page-in (PagerBacked::supply_shared_pages, before supplying the rest of the range) shares each frozen page that lies within the file's size, filling the cache from the read on a miss. Caches of owners no longer frozen are dropped when a volume is removed.
Tests: zircon unit tests (shared_pages_unittest), zircon-core-tests (shared_pages), fxfs test_snapshot_frozen_pages, fxdax tests/shared_pages (one page per block across a volume and its snapshot; DAX maps shared pages and a store copies them). Then both hosts' suites and guest tests, docs, deviation notes.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design changes from the first plan: the caches are one per frozen owner rather than per filesystem, since a frozen owner's data never changes while it is frozen, so nothing needs invalidating; a cache is dropped once its owner is no longer frozen (pruned when a volume is removed). Borrowers copy a borrowed page eagerly in require_owned_page (all writes and commits go through it) rather than only on the DIRTY ack; dirty_pages also copies, for DIRTY acks over ranges with borrowed pages. Lent pages are kept Clean and unpinned by revoking before the lender dirties or pins them, so borrowers never see a lender's writes. An object only lends or borrows (fixed at its first loan), which makes lender-then-borrower the only lock nesting. Cache pages are indexed by device block (block number times page size) so 16 KiB pages work over 4 KiB blocks. Only unencrypted files, whole pages within the file size and within one fully written extent of one frozen owner are shared.
Validation: zircon shared_pages_unittest 18/18, zircon-core-tests shared_pages 9/9, fxfs test_snapshot_frozen_pages, fxdax tests/shared_pages 2/2 (fail with sharing disabled: 128 vs 64 pages, 72 vs 64). Workspace suites: macOS 1856 passed, Linux 1859 passed. upstream-changes.py --check passes.

Guest tests (virtiofs-fxdax, -nodax, -volumes) pass on macOS and Linux. The DAX criterion is shown in-process by fxdax's tests/shared_pages through a DaxWindow, as libkrun has no snapshot API for a guest test.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Volumes that share extents now share their cached pages. The kernel gains borrowed pages: zx_pager_supply_shared_pages supplies a pager VMO with another pager VMO's clean pages, which stay the lender's (one page, counted and aged once). Borrowers copy before writing, dirtying or pinning, and give pages back rather than freeing them; lenders take them back before dirtying, pinning, evicting or removing them. Fxfs finds a file's pages on frozen-owner extents (Allocator::frozen_ranges, DataObjectHandle::frozen_pages), and the platform layer shares them from a cache VMO per frozen owner (SharedBlocks) at page-in. Verified by zircon unit tests (18), zircon-core-tests (9), an fxfs test, and fxdax tests showing a volume and its snapshot reading a 1 MiB file use one page per block and that DAX stores copy the page (both fail with sharing disabled); full suites and guest tests pass on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
