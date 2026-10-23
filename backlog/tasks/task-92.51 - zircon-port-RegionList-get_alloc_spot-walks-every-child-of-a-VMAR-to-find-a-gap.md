---
id: TASK-92.51
title: >-
  zircon port: RegionList::get_alloc_spot walks every child of a VMAR to find a
  gap
status: To Do
assignee: []
created_date: '2026-10-23 19:20'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 357000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-92.50 (r6/zircon-map). crates/zircon/src/vm/vm_address_region.rs's RegionList::get_alloc_spot walks every child of the VMAR looking for a gap, where upstream's RegionList is a B-tree augmented with each subtree's largest gap (vm_address_region_observer.h) and finds one in log time. Window::map's SPECIFIC mappings do not take this path, but any non-specific mapping into a VMAR with many children does, and the module header does not list the deviation.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An allocation in a VMAR of N children finds its spot in O(log N), as upstream's, shown by a test that counts the children visited
- [ ] #2 The deviation is gone or documented with an fxdax: marker
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
