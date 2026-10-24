---
id: TASK-79.103
title: >-
  A control ForkStore of a snapshot point extentd keeps open fails on a fence
  from another host
status: To Do
assignee: []
created_date: '2026-10-24 03:39'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 374000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found in round 2b (TASK-92.5). extentd now keeps a snapshot's point open for its restores; restores were made to cope with a point whose branch another host's lease fenced, but stores::fork (control ForkStore of a kept point) still fails on such a fence.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A ForkStore of a kept point another host's lease fenced reopens it and forks, or fails with an errno saying so, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
