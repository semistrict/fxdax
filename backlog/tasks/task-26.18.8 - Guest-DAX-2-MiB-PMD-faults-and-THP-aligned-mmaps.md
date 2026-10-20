---
id: TASK-26.18.8
title: 'Guest DAX: 2 MiB (PMD) faults and THP-aligned mmaps'
status: To Do
assignee: []
created_date: '2026-10-15 08:19'
updated_date: '2026-10-20 22:06'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: deferred: the bench has no mmap-seq-read workload, and Fxfs is ahead of ext4 on mmap-rand-read already (0.70x, gce-fs-bench-perf-base-2026-10-20T2051Z).
<!-- SECTION:NOTES:END -->
