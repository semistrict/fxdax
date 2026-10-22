---
id: TASK-26.18.9
title: 'Guest DAX: 2 MiB read faults for files written in small pieces (SQLite)'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-21 15:19'
updated_date: '2026-10-21 20:44'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: low
type: enhancement
ordinal: 165000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-26.18.8 gave guest Fxfs 2 MiB (PMD) read faults on DAX mappings: a large file written sequentially now faults 5.5 times per MiB, like ext4-dax-pmem. A file written a few KiB at a time does not: the bench's sqlite-mmap-scan (SQLite with mmap_size scanning a 256 MiB table) takes 256 faults per MiB on fxfs-guest-dax against ext4's 3.7 (docs/measurements/gce-fs-bench-mmap-pmd-2026-10-21T1045Z), though it is already faster (703 vs 312 MiB/s). Small writes go to the allocator's staging extents, shared by files, whose pieces the guest maps where the extent's region is (fxfs_dax_move), at any window offset; debug logs showed runs of 1 to 6 pages per 2 MiB chunk. Find the mechanism with evidence, then a fix that keeps what regions are for (small files sharing extents take no MAP of their own; the window maps each extent once).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The mechanism by which a small-write file's chunks are not one aligned run in the window is shown, with evidence
- [ ] #2 sqlite-mmap-scan's faults per MiB on fxfs-guest-dax come near ext4-dax-pmem's on GCE (3 iterations), its throughput not lower, the other io workloads unchanged
- [ ] #3 fxfs-guest-dax, fxfs-guest-checks-dax and fxfs-guest pass, with no kernel warnings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Diagnose with tagged guest debug logs on GCE: for SQLite's chunks, the pieces the window maps (extent, offset in it, length, file offset), and the runs a PMD fault finds.
2. Weigh fixes by what the evidence shows (alignment of staging regions; remapping a scattered chunk into its own space on a PMD fault; the allocator keeping one file's small writes in order), and the gain at stake (faults' share of the scan's time).
3. Implement, remove the logs, measure on GCE (io group, 3 iterations), run the guest tests.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-21: paused for the sandbox team's pilot blockers (TASK-57). A diagnostic run (tagged [DEBUG-small] logs of the pieces of chunks with 16 or more, removed from the tree) was under way; earlier logs showed SQLite's chunks in runs of 1 to 6 pages. Working guess: SQLite's database pages are written in order but interleaved with its rollback journal, so the allocator restages at each switch of owner and a chunk is a few runs in different staging extents; a fix could map such a chunk into its own aligned space on its first PMD fault.
<!-- SECTION:NOTES:END -->
