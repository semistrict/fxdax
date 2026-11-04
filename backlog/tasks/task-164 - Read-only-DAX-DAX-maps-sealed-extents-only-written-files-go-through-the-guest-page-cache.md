---
id: TASK-164
title: >-
  Copy-on-write DAX: the window maps sealed extents read-only; a store gets a
  private page, allocated at writeback
status: To Do
assignee: []
created_date: '2026-11-04 07:34'
updated_date: '2026-11-04 07:58'
labels: []
dependencies: []
priority: high
type: task
ordinal: 609000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decided by the user on 2026-11-04 (docs/reviews/2026-11-04-first-principles.md, doc-13). The window maps only sealed extents (or zeros), read-only. A guest's first store to such a page gets a private host copy at the same window address, nothing moving; Fxfs allocates extents at writeback (delayed allocation), fsync writing dirty window pages as the no-DAX path writes and committing after the bytes; once a page's new extent is sealed the guest maps it back to it. Per page, not per file: a 400 GB file mapped read-write with 5% written keeps the rest on DAX, only written pages costing memory, up to private_max per VM (a quarter of the window by default, the user's choice). Files created in a mount stay page-cache inodes until read in again. Removes the gate, seals' write-protects, moves, staging and restaging, lagging pages, held copies, the open-page budget and ENOMEM, seal asks, writable MAPs, txn_map_write*, ExtentAccess, patches 0005/0008/0009 and the dax, dax-region and extent-open specs. Stages in doc-13; subtasks below.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A design doc settles the per-inode switch, mmap of written files, and what of the window, the host and Fxfs goes
- [ ] #2 A prototype passes the DAX and crash suites, and the perf gate shows writes at no-DAX speed and base reads at today's DAX speed
- [ ] #3 The writable DAX path and what only it needed are deleted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04, the user: the design must serve a Redis-like workload. A 400 GB file mmapped read-write (MAP_SHARED), 5% of it written in this fork: the unwritten 95%, including blocks written in earlier forks (sealed now), stays DAX, and only the written pages cost extra memory. So the switch can't be per file: it is per page within one mapping. Candidates: host copy-on-write with writeback allocation (the review's option 1b), or a guest-side per-page mix of DAX entries and page-cache folios. The design doc decides.

2026-11-04, the user: per page by host copy-on-write over per file; private_max a quarter of the window, configurable; density targets set from the density gate's measurements, not fixed now.
<!-- SECTION:NOTES:END -->
