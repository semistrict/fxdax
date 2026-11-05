---
id: TASK-160.4
title: 'Guest DAX: one module owns a range''s page state'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 03:58'
labels:
  - structure
dependencies:
  - TASK-164
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 575000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Re-scoped 2026-11-04 after writable DAX was deleted (doc-13): the writable and lagging bitmaps and `behind` are gone. What is left of a range's page state is copy on write's: four bitmaps, `mapped` (a page has a window page: an extent's, zeros, or private), `zeros` (that window page is zeros: a hole or past the end), `private` (the host holds it the VM's own, counted against private_max) and `dirty` (in the file's dirty set; dirty implies private). TASK-164.9 gave each of private and dirty one changing function, but the bitmaps are still a field of struct fxfs_dax_range that any of dax.c reads and writes: resize_end sets `zeros` and clears `mapped` itself after fxfs_dax_forget, a slice sets `mapped` itself, writeback swaps `dirty` word by word, rebind, map_missing and iomap_read walk the bitmaps with find_next_* idioms, and dirty implies private, private implies mapped and zeros implies mapped hold by convention, checked at two places. Proposal: a module of its own (pages.c), whose struct dax.c can't look into, holding the bitmaps and their invariants, with a named transition for each way a page's state changes (filled by a MAP, dirtied by an iomap, swapped out by writeback, rebound by a MAP, cut by a truncate's MAP of zeros, unmapped) and the queries dax.c needs; dax.c keeps the tallies, the budget and the device requests around each.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 No bitmap of a range is read or edited outside one module (pages.c), which dax.c sees only through named transitions and queries
- [ ] #2 The module checks its invariants (dirty implies private, private and zeros imply mapped) at each transition
- [ ] #3 guest/sim's mutants of page state, rewritten against the module, are still caught: task-164.4-forget-before-free, task-164.4-rebind-dirty, task-164.4-writeback-unowned, task-164.9-private-failure-ignored, task-164.4-swap-shared, task-164.4-private-uncounted, unmapped-page-read-as-hole
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. guest/linux/fs/fxfs/pages.{c,h}: struct fxfs_dax_pages, opaque; transitions fill, dirty, swap, rebind, cut, unmap; queries; invariants WARNed.
2. dax.c: range->state in place of the four bitmaps; every bitmap use through the module. Kbuild and guest/sim build it.
3. Rewrite the page-state mutants against it; mutants.sh --check; zloop.sh -s 3000; every mutant run.
4. README and dax.c's header say so.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04: waits for TASK-164 (read-only DAX), which changes what this covers; re-scope it then.

2026-11-04 (task-160-4, 1cc2813b7). Design: guest/linux/fs/fxfs/pages.{c,h}, struct fxfs_dax_pages opaque to dax.c (one allocation, four bitmaps: mapped, zeros, private, dirty). Transitions, one per thing that happens to a page: fill (a range's own MAP, or a slice's region; WARNs if mapped already), dirty (the iomap, its PRIVATE done; atomic, private before dirty, under the semaphore shared), swap (writeback takes the dirty set into its batch), rebind (a rebind's MAP over clean private pages; WARNs on dirty), cut (a truncate's MAP of zeros: zeros and mapped, private and dirty dropped), unmap. Each returns what it changed (struct fxfs_dax_change) for dax.c to count; the tallies, the budget, Fxfs's room for dirty pages and the requests stay dax.c's (fxfs_dax_dropped replaces fxfs_dax_forget, fxfs_dax_mapped the old mapped+rebound pair). Exclusive transitions check dirty implies private, private and zeros imply mapped over their words. Queries: would_dirty, any_dirty, any_mapped, mapped, next_unmapped, mapped_run, unwritten, unwritten_run, next_rebindable. dax.c names no bitmap now (rg range->(mapped|private|dirty|zeros): none). One change of behaviour, the invariant's: a truncate's MAP of zeros marks the pages mapped, which resize_end left unmapped if an earlier failed MAP had unmapped them (a redundant MAP later, no more).
Structure: the class (a bitmap edited by code that doesn't keep the rules) can't recur: the compiler refuses dax.c a field of the state.
Verified: guest/sim check.sh -s 300 0 failed; zloop.sh -s 3000 passed (199 s); mutants rewritten against the module (task-164.2-writeback-holds-no-range, task-164.4-forget-before-free, task-164.4-rebind-dirty (now in pages.c), task-164.4-writeback-unowned, task-164.9-private-failure-ignored, unmapped-page-read-as-hole), mutants.sh --check 28/0 broken; page-state mutants each caught at once (forget-before-free cow_truncate_held; rebind-dirty pages.c's WARN in cow_rebind_against_stores; writeback-unowned and private-failure-ignored the host's refusal; swap-shared cow_swap_during_write; private-uncounted read_mappings_stay; unmapped-page-read-as-hole hole_mapped_then_written; writeback-holds-no-range ASan in read_pressure).
<!-- SECTION:NOTES:END -->
