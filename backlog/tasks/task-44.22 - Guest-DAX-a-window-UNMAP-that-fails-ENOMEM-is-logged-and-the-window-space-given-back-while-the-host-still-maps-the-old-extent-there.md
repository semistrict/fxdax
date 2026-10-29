---
id: TASK-44.22
title: >-
  Guest DAX: a window UNMAP that fails (ENOMEM) is logged, and the window space
  given back while the host still maps the old extent there
status: In Progress
assignee: []
created_date: '2026-10-29 21:33'
updated_date: '2026-10-29 22:26'
labels:
  - dax
  - bug
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 478000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r269 (fxfs-guest-faults-dax, the debug kernel failing the writers' kernel allocations): 'fxfs: unmapping the DAX window at 33c0000 failed: -12'. fxfs_dax_put_region, fxfs_dax_unmap_range and the truncate path (dax.c ~1640, ~1703, ~4727) pr_warn on fxfs_dax_device_unmap's failure, then give the window space back (fxfs_dax_give_window) as if it were unmapped. The host keeps mapping the old extent's pages there: the next range or region the space goes to reads them wherever it reads a page it hasn't mapped yet, and a user's mapping the kernel didn't replace still maps them. A failure of the device request's allocation needs a retry (it must succeed, as a seal's barrier must), or the space kept out of use until an UNMAP of it succeeds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A failed UNMAP is retried until it succeeds, or its window space kept from reuse until it does
- [ ] #2 fxfs-guest-faults-dax with the DAX code's allocations failing shows no window page mapping another file's data
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-29: fxfs_device_unmap allocates its request with __GFP_NOFAIL (failslab skips those), and fxfs_dax_give_unmapped keeps the window space of an UNMAP that fails anyway from use, counted in window_lost_bytes; fxfs-guest-faults-dax asserts window_lost_bytes 0. GCE r255 (debug kernel) round 1: faults-dax's writer panicked writing make-it-fail=0 (ENOMEM: the write allocates), now retried; checks-dax failed on KCSAN, fxfs_dax_cow's unlocked find_next_bit against readers' test_and_clear_bit, benign (a hint; only readers clear), marked data_race. Rerunning (r255-unmap3).

GCE r255 (debug kernel) at 7c9948612: fxfs-guest-faults-dax 3 rounds passed (window_lost_bytes 0 asserted), fxfs-guest-checks-dax 3 rounds passed (after marking fxfs_dax_cow's and fxfs_dax_grow_ahead's KCSAN races).
<!-- SECTION:NOTES:END -->
