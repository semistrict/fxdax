---
id: TASK-26.18.8
title: 'Guest DAX: 2 MiB (PMD) faults and THP-aligned mmaps'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-21 10:59'
labels: []
dependencies: []
parent_task_id: TASK-26.18
priority: low
type: enhancement
ordinal: 62000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ext4 DAX maps 2 MiB at a time where it can (ext4_dax_huge_fault) and aligns mmaps with thp_get_unmapped_area; Fxfs maps 4 KiB pages only, one fault each, so large mapped files take 512 faults per 2 MiB.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The guest DAX vm_operations implement huge_fault for PMD-aligned, extent-backed ranges
- [x] #2 File operations set get_unmapped_area = thp_get_unmapped_area
- [x] #3 mmap-seq-read on GCE improves, and fxfs-guest-dax passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bench: mmap-seq-read (fio mmap engine, one sequential pass of the data file in a fresh mapping, 1 MiB blocks) and an SQLite workload with PRAGMA mmap_size (full-table scans in fresh processes), each recording the guest's page faults; baseline on GCE against ext4-dax-pmem.
2. Feasibility: how Fxfs lays out a large file's extents and places them in the DAX window: a PMD fault needs the file offset and the window address 2 MiB-aligned over one extent.
3. huge_fault for PMD-aligned extent-backed ranges in the guest's DAX vm_operations; get_unmapped_area = thp_get_unmapped_area.
4. Measure again on GCE; fxfs-guest-dax and fxfs-guest-checks-dax pass.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: deferred: the bench has no mmap-seq-read workload, and Fxfs is ahead of ext4 on mmap-rand-read already (0.70x, gce-fs-bench-perf-base-2026-10-20T2051Z).

Bench gained mmap-seq-read and sqlite-mmap-scan with faults/MiB. Baseline (T0924Z): Fxfs 2,000 MiB/s, 261 faults/MiB; ext4 9,481, 5.6. huge_fault alone (T0947Z): 2 of 3 iterations at 2 MiB; the third every chunk two 1 MiB runs (debug logs: 'run short, page 0 end 256 of 512, moved'): whole 1 MiB extents mapped at their regions. Fixed: only parts of extents (staging) move. Final (T1045Z): 10,240 MiB/s x3, 5.5 faults/MiB, 1.00x ext4. Guest tests fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest pass, consoles clean. DoD #1: this change touches only the guest kernel and the guest test crate, which the workspace's nextest doesn't build; the guest tests are its tests. sqlite-mmap-scan still pages (256/MiB): SQLite's 4 KiB writes land in staging extents, whose regions sit unaligned; proposed as follow-up.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Guest Fxfs's DAX mappings take 2 MiB read faults (huge_fault, read-only; stores fall back to pages for the page-by-page sealer), VM_HUGEPAGE and 2 MiB-aligned mmaps; and a file's whole extents map in their chunks' own window space, so a large file's chunks are aligned runs. mmap-seq-read 2,000 -> 10,240 MiB/s (1.00x ext4-dax-pmem), faults 261 -> 5.5 per MiB; guest tests pass. SQLite through mmap still faults pages (follow-up).
<!-- SECTION:FINAL_SUMMARY:END -->
