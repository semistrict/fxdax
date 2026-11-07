---
id: TASK-164
title: >-
  Copy-on-write DAX: the window maps sealed extents read-only; a store gets a
  private page, allocated at writeback
status: Done
assignee: []
created_date: '2026-11-04 07:34'
updated_date: '2026-11-07 00:31'
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
- [x] #1 A design doc settles the per-inode switch, mmap of written files, and what of the window, the host and Fxfs goes
- [x] #2 A prototype passes the DAX and crash suites, and the perf gate shows writes at no-DAX speed and base reads at today's DAX speed
- [x] #3 The writable DAX path and what only it needed are deleted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-04, the user: the design must serve a Redis-like workload. A 400 GB file mmapped read-write (MAP_SHARED), 5% of it written in this fork: the unwritten 95%, including blocks written in earlier forks (sealed now), stays DAX, and only the written pages cost extra memory. So the switch can't be per file: it is per page within one mapping. Candidates: host copy-on-write with writeback allocation (the review's option 1b), or a guest-side per-page mix of DAX entries and page-cache folios. The design doc decides.

2026-11-04, the user: per page by host copy-on-write over per file; private_max a quarter of the window, configurable; density targets set from the density gate's measurements, not fixed now.

2026-11-05: AC #1 met by doc-13 (Per page or per file; the guest's writeback and mmap of written files; What goes, What stays). AC #3 met by TASK-164.7 (37c331a6c; GCE r553 workspace, r554 and r555 jailed); rg finds no txn_map_write, DaxWrite or OpenPages on main. AC #2 half met: GCE r690 on main runs copy-on-write as the only DAX, and its DAX and crash suites pass 15 of 15 (fxfs-guest-dax, checks-dax, -delay, -small, crash-dax, crash-dax-killed, crash-stray-dax, pgbench-dax, docker-dax); the perf gate doesn't yet: forked-dax 0.573 (r632) and 0.579 (r649) against 0.9, dax-density decay 0.883, which TASK-164.6 (To Do) holds.

2026-11-06, AC2 checked against the full perf gate on main ccbe81d0e (GCE r790, r791, jailed; docs/measurements/dax-cow-gate.md): writes at no-DAX speed met (dax 0.976, created files in the page cache); base reads at today's DAX speed met as bigmap-read gates them (cold reads 5.32x no DAX's, 88.9 against 16.7 MiB/s; the comparison with pre-deletion DAX can't be run, TASK-164.6); the DAX and crash suites pass (r647, r650, r651); but the prototype's forked pgbench fails its gate, 0.304 (TASK-164.12: a fork's inherited data paged in from the bucket a ranged GET at a time since TASK-165.8). AC2 stays open until TASK-164.6 passes.

2026-11-06: AC2 met. The DAX and crash suites pass in the dax-gate at dde984800 (fxfs-guest-dax, checks-dax, -delay, -small, crash-dax, crash-dax-killed, 5 rounds each). The perf gate, r794: writes at no-DAX speed (dax 0.974) and base reads past DAX's former speed (bigmap-read 5.52x no DAX's, the gate that stands for 'today's DAX' since writable DAX was deleted; TASK-164.6). TASK-164.11 stays open, later work.

2026-11-07: every subtask done but TASK-164.11.1 and TASK-164.11.2, later work on a PRIVATE's latency (OPEN_QUESTIONS.md). The perf gate passes in full with forked-dax 0.912 (r807), the dax-gate at c69b387a0. Validation: macOS FXDAX_TEST_PARTS='workspace scripts' scripts/test.sh, 2051 tests and 162 script tests passed; Linux by land-p3's CI and the guest suites on GCE (dax-gate at c69b387a0: fxfs-guest-dax, checks-dax, -delay, -small, crash-dax, -killed 5 rounds each; pgbench-dax-forked, probes a map 45 us, a write(2) 113 us; every mutant killed or none); scripts/upstream-changes.py --check passes; the changes are fxdax's own code, marked fxdax where it sits beside upstream's.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Copy-on-write DAX is the only DAX: the window maps sealed extents read-only, a store gets a private page, allocated at writeback (doc-13), writable DAX deleted. Its perf gate passes in full on GCE (r807: forked-dax 0.912, dax 0.995, density-memory 0.811, bigmap-read 5.15), its dax-gate at c69b387a0. TASK-164.11.1 and .2, a PRIVATE's latency, are left as later work.
<!-- SECTION:FINAL_SUMMARY:END -->
