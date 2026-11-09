---
id: TASK-286
title: >-
  Debug kernel KCSAN: data race in folio_clear_dirty_for_io /
  fxfs_vfs_page_mkwrite fails fxfs-guest-checks-dax-small
status: To Do
assignee: []
created_date: '2026-11-09 07:51'
labels:
  - bug
  - flaky
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE t257dbg1 (TASK-257's branch on main 142195ae2, debug kernel, fxdax-keep-n2-standard-4-us-west1-a-1), fxfs-guest-checks-dax-small round 3 of 5 failed on the console: 'BUG: KCSAN: data-race in folio_clear_dirty_for_io / fxfs_vfs_page_mkwrite': a marked read-write of the folio's flags (8 bytes) in fxfs_vfs_page_mkwrite (do_wp_page, a write fault), against a plain read in folio_clear_dirty_for_io (writeback_get_folio, writeback_iter, fxfs_vfs_writepages, wb_workfn); value changed 0x000fffe00000412d -> 0x000fffe0000041ad (PG_dirty set). Near the torture check. Decide whether it is the kernel's benign folio-flag read (as TASK-44.8.1's reviewed readers) to be listed, or a page_mkwrite that dirties a folio writeback is cleaning without the folio lock.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The race reviewed: either fixed in fxfs_vfs_page_mkwrite or documented and listed as the kernel's benign reader
- [ ] #2 fxfs-guest-checks-dax-small passes 20 rounds on the debug kernel with no KCSAN report
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
