---
id: TASK-117
title: >-
  Guest DAX: resize_end zeroes moved[] past EOF without releasing the regions'
  references (suspected)
status: To Do
assignee: []
created_date: '2026-10-28 12:45'
labels:
  - bug
dependencies: []
ordinal: 434000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28): fxfs_dax_resize_end zeroes moved[] for pages past the new end but leaves the regions' references to a later prune that may never run (see the relocated/prune task). Unverified.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Confirmed or refuted with a truncate-after-write test checking region release
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
