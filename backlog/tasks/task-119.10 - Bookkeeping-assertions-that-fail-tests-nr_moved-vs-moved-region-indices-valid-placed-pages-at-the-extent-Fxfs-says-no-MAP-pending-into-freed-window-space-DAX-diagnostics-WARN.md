---
id: TASK-119.10
title: >-
  Bookkeeping assertions that fail tests: nr_moved vs moved[], region indices
  valid, placed pages at the extent Fxfs says, no MAP pending into freed window
  space; DAX diagnostics WARN
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 448000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable/Astra: at place, ask Fxfs (fxfs_dax_extents) where each page is and WARN unless it is the run mapped; assert nr_moved and moved[] agree at relocate/prune/resize_end; a bounded event history per page (extent, seal count, MAP id) printed on failure. The runner fails tests on WARNING:, not pr_warn.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Assertions behind a debug param, on in checks-dax-delay
- [ ] #2 A hand mutant breaking each is caught by its assertion
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
