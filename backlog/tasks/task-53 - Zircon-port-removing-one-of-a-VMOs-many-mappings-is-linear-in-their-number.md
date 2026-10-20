---
id: TASK-53
title: 'Zircon port: removing one of a VMO''s many mappings is linear in their number'
status: To Do
assignee: []
created_date: '2026-10-20 14:57'
labels:
  - zircon
  - perf
dependencies: []
priority: medium
ordinal: 161000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
VmCowPagesLocked keeps a VMO's mappings in a Vec (crates/zircon/src/vm/vm_cow_pages.rs mappings), and remove_mapping finds one by a linear scan, so tearing down N mappings of one VMO takes O(N^2): found by TASK-12's test, whose window mapped a 69,626-page file a page at a time, reversed; dropping the window (VmAspace::destroy) took over 12 minutes (debug build, GCE). Upstream Zircon keeps them in an intrusive list (O(1) removal). A DAX window mapping a file's extents in many pieces, or many windows of one file, pays it on every unmap.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Removing a mapping of a VMO with N mappings is O(1) or O(log N)
- [ ] #2 A test maps one VMO 100,000 times and unmaps them all in seconds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
