---
id: TASK-26.12
title: 'Host: DAX_WRITE in fxdax''s ExtentStore and libkrun''s extent device'
status: Done
assignee: []
created_date: '2026-10-14 14:46'
updated_date: '2026-10-14 15:06'
labels: []
milestone: m-3
dependencies:
  - TASK-26.11
parent_task_id: TASK-26
type: feature
ordinal: 48000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
MAP_WRITE (doc-4, TASK-26.11) on the host: an open extent's pages are memory of fxdax's in-process kernel, mapped writably into the DAX window, which the next COMMIT writes to the store and seals, after which the same pages are the sealed extent's in the shared cache, mapped read-only. libkrun's device offers DAX_WRITE when the store's window has it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 MAP_WRITE maps a new or open extent of the device's branch writably, and stores through the window are the extent's data once a COMMIT seals it, shown by a test reopening the store
- [x] #2 After the COMMIT the window's mapping of it is read-only, and other windows mapping the sealed extent share its pages
- [x] #3 MAP_WRITE of a sealed extent, or another branch's, fails with EPERM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ExtentStore keeps open extents mapped writably as VMOs of MAX_EXTENT (READ/WRITE use them while open). 2. COMMIT writes them to the store, seals them, remaps their window mappings read-only (Dax::make_read_only) and keeps the VMOs as the sealed extents' cache entries. 3. libkrun's device offers DAX_WRITE and passes MAP_WRITE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation: fxdax extents::tests::maps_open_extents_writably (stores through the window survive reopening; the mapping is read-only after COMMIT; a copy's window maps the same cached pages; EPERM for sealed or other branches' extents); krun-devices extent worker test (MAP_WRITE flag, EINVAL for unknown flags). scripts/test.sh 1865/1865 and scripts/test-linux.sh 1869/1869, upstream-changes.py --check passes. Commits 697fdd1, 76a9a68; libkrun fc42259.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
MAP_WRITE maps the guest's open extents writably from in-process-kernel VMOs, which COMMIT seals into the store and the shared cache, remapping the window read-only; libkrun offers DAX_WRITE. Verified by fxdax's extents tests and libkrun's extent worker test, on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
