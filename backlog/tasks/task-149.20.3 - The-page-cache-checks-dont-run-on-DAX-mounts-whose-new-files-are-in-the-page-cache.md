---
id: TASK-149.20.3
title: >-
  The page-cache checks don't run on DAX mounts, whose new files are in the page
  cache
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 13:36'
updated_date: '2026-11-07 20:42'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149.20
priority: high
type: task
ordinal: 668000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-149.20. fxfs-guest-checks' large_folios, scattered_writeback, partial_writeback and writeback_out_of_memory run in the page-cache suites alone (Suites::PageCache, guest/tests/test_cases/src/test_fxfs_guest_checks/mod.rs: 'Files on DAX are in no page cache'). Since copy-on-write DAX (doc-13, TASK-164) a DAX mount's new files are in the page cache till read in again (cow_created_in_page_cache, TASK-44.15's notes), so those writeback paths run on DAX mounts too, unchecked there: as test_short_copy, which covered a data loss bug, was off on DAX (TASK-149.20's description).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The four checks run in the DAX suites too (Suites::All, or a variant whose files are made on the DAX mount), on GCE
- [x] #2 What they find on DAX is fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. guest_fxfs::drop_pages: write back and drop a file's pages, keeping its inode, so a file a check makes on a DAX mount stays the page cache's (copy-on-write DAX makes created files page-cache inodes until read in again).
2. The four checks drop pages instead of evicting before their writes, assert the file isn't DAX (in_page_cache), and evict only to read back what reached the device.
3. Suites::All for the four; Suites::PageCache removed (unused); the table's order test updated.
4. GCE: fxfs-guest-checks, -dax, -dax-delay, -dax-small (release, r960), and the four alone on the debug kernel for writeback_out_of_memory (r961).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done (commit 2927d03ae on land-r2). The four checks drop pages and keep the inode (guest_fxfs::drop_pages), assert the file isn't DAX (in_page_cache), and run in Suites::All; Suites::PageCache is gone. GCE r960, release: fxfs-guest-checks, -dax, -dax-delay and -dax-small passed (70-87 s). GCE r961, debug, the four alone: -checks, -dax and -dax-small passed; on DAX writeback_out_of_memory had 5 of 64 fsyncs' allocations fail. r963: -checks-dax passed again on the rebased tree. On DAX they found nothing to fix or file.

CI 42410439072 on land-r2 4ab1eb0f2: all 15 jobs green, macOS and Linux workspace, guest, guest-sim and its sweep, and clippy scripts among them.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
large_folios, scattered_writeback, partial_writeback and writeback_out_of_memory run in every suite, DAX included. Each keeps its file the page cache's (drop pages, not the inode, and assert !is_dax), since copy-on-write DAX makes created files page-cache inodes. Verified on GCE r960 (release, 4 suites), r961 (debug, fault injection) and r963; nothing found on DAX.
<!-- SECTION:FINAL_SUMMARY:END -->
