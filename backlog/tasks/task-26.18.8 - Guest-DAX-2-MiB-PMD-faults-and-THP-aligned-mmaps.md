---
id: TASK-26.18.8
title: 'Guest DAX: 2 MiB (PMD) faults and THP-aligned mmaps'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-15 08:19'
updated_date: '2026-10-21 09:23'
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
- [ ] #1 The guest DAX vm_operations implement huge_fault for PMD-aligned, extent-backed ranges
- [ ] #2 File operations set get_unmapped_area = thp_get_unmapped_area
- [ ] #3 mmap-seq-read on GCE improves, and fxfs-guest-dax passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
