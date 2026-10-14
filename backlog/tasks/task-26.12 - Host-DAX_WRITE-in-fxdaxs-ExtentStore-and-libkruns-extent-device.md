---
id: TASK-26.12
title: 'Host: DAX_WRITE in fxdax''s ExtentStore and libkrun''s extent device'
status: To Do
assignee: []
created_date: '2026-10-14 14:46'
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
- [ ] #1 MAP_WRITE maps a new or open extent of the device's branch writably, and stores through the window are the extent's data once a COMMIT seals it, shown by a test reopening the store
- [ ] #2 After the COMMIT the window's mapping of it is read-only, and other windows mapping the sealed extent share its pages
- [ ] #3 MAP_WRITE of a sealed extent, or another branch's, fails with EPERM
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
