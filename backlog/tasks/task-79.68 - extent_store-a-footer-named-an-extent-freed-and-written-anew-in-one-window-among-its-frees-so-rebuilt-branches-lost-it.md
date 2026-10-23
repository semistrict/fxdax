---
id: TASK-79.68
title: >-
  extent_store: a footer named an extent freed and written anew in one window
  among its frees, so rebuilt branches lost it
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 319000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.1 (round 1). An extent freed while open, then written again before its commit, was named by its segment's footer both among the extents the commit sealed and among those it freed. Readers of footers apply the frees after the extents (Store::rebuild, and recovery in object storage), so the branch they rebuilt lacked the extent. Footers uploaded before the fix by stores that did this still name such extents (no migration).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A footer never names among its frees an extent its window's last commit holds
- [ ] #2 Store::rebuild and object-storage recovery of such a window keep the extent, pinned by tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
