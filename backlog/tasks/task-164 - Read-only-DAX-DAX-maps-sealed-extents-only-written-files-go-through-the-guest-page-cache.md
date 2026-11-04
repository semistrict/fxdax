---
id: TASK-164
title: >-
  Read-only DAX: DAX maps sealed extents only; written files go through the
  guest page cache
status: To Do
assignee: []
created_date: '2026-11-04 07:34'
updated_date: '2026-11-04 07:35'
labels: []
dependencies: []
priority: high
type: task
ordinal: 609000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Decided by the user on 2026-11-04 from the first-principles review (docs/reviews/2026-11-04-first-principles.md, section 1, option a). Writable DAX (a window page being the open extent's block, every fsync sealing every open extent) costs DAX ~0.55x no-DAX under pgbench, ~47% of bug tasks, 37-39 of 65 dead-ends and ~25k lines, while density comes from reading shared sealed base pages. New design: DAX maps only sealed extents, read-only; a file switches to the guest page cache at its first write (per inode); writes take the no-DAX path. Removes the gate, seals' write-protects, moves on store, restaging, the open-page budget and ENOMEM, refused stores, exit 159, most fs/dax.c patches, specs dax/dax-region/extent-open and most of guest/sim; host-side open extents in the window go. Plan: a design doc first (the per-inode switch: Linux allows changing DAX mode only at eviction; mmap of a file being written; reads of a written file's sealed blocks; followers frozen), then a prototype behind a mount option judged by the perf gate (pgbench-dax vs no-DAX, first reads of a base, density memory per VM) and the crash and DAX suites, then deletion of the writable path.
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
<!-- SECTION:NOTES:END -->
