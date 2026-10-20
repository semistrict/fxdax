---
id: TASK-53
title: 'Zircon port: removing one of a VMO''s many mappings is linear in their number'
status: Done
assignee: []
created_date: '2026-10-20 14:57'
updated_date: '2026-10-20 19:03'
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
- [x] #1 Removing a mapping of a VMO with N mappings is O(1) or O(log N)
- [x] #2 A test maps one VMO 100,000 times and unmaps them all in seconds
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
af12ab1: mappings in a BTreeMap keyed by the mapping's address (O(log N) removal). guest_physical_aspace_many_mappings_of_one_vmo maps one VMO 100,000 times, unmaps half in reverse, destroys the rest: 12.1 s before, 0.5 s after (macOS debug). Linux workspace and DAX guest suites pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Removing one of a VMO's many mappings is O(log N) (BTreeMap by address); 100k mappings map and unmap in half a second.
<!-- SECTION:FINAL_SUMMARY:END -->
