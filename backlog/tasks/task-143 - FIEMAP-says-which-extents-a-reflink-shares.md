---
id: TASK-143
title: FIEMAP says which extents a reflink shares
status: To Do
assignee: []
created_date: '2026-10-31 22:07'
labels: []
dependencies: []
priority: low
ordinal: 493000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
generic/372, 414, 578 and 702 expect FIEMAP_EXTENT_SHARED, which fxfs_vfs_fiemap never sets. Allocator::shared_owners shows the allocator can answer per store; per extent it needs a design.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 FIEMAP_EXTENT_SHARED is set of a reflinked extent, generic/372, 414, 578, 702 pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
