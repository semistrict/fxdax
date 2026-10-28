---
id: TASK-123
title: >-
  Guest DAX: a read of a chunk mapped 2 MiB at once dies of SIGBUS after a store
  moved one of its pages
status: Done
assignee:
  - '@claude'
created_date: '2026-10-28 21:22'
updated_date: '2026-10-28 21:50'
labels:
  - bug
dependencies: []
priority: high
ordinal: 457000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-10-28 by mapped_after_write on main (805149a7c, GCE r208): a 4 MiB file written by one write(2) and fsynced, mapped and read whole (a read fault maps the first chunk with one PMD, its pages in a row in the window), then a store to its second page (which moves the page: the fault falls back to pages, the PMD zapped), then the mapping read again: SIGBUS, the process killed (memcmp at the mapping first byte). Likely: only the moved page is relocated (fxfs_dax_cow), so only its fault says IOMAP_F_SHARED and replaces the kernel PMD entry; the other pages faults meet the real PMD entry, which grab_mapping_entry does not downgrade for a PTE fault, and fail.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 mapped_after_write passes 5 rounds on checks-dax
- [x] #2 A hand mutant restoring the cause is killed by it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Mark every mapped page of a range a PMD fault mapped relocated when one of them moves, so each page's next fault replaces the PMD entry (IOMAP_F_SHARED). 2. Check mapped_after_write (read whole, store moves a page, read whole). 3. Hand mutant pmd_row_unrelocated.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Fix 6fa74947 (test/mapped-after-write): range->huge set by a fault iomap of PMD_SIZE; fxfs_dax_relocate relocates every mapped page once a page of a huge range changes place. GCE r208: main without the fix died of SIGBUS in round 1 of mapped_after_write; with it mapped_after_write,mmap_write,truncate_mapped,zero_mapped passed 3/3, mapped_after_write 2/2 more (5 total); fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax 2/2 each (run-20261028T21xx, results /tmp/fxdax-r208-maw4). Mutant pmd_row_unrelocated killed (r207). scripts/upstream-changes.py --check passes; scripts/tests guest mutants pass. cargo nextest --workspace not run: the change touches guest/linux and guest/tests and scripts only.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A page moving out of a 2 MiB row a PMD fault mapped now relocates every mapped page of the range, so each page's next fault replaces the PMD DAX entry; reads of the row's other pages died of SIGBUS. Verified on GCE: new check mapped_after_write fails on main in round 1 and passes 5/5 with the fix; DAX suites pass; hand mutant pmd_row_unrelocated killed.
<!-- SECTION:FINAL_SUMMARY:END -->
