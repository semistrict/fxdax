---
id: TASK-79.72
title: >-
  A dead writer's fork segment landing after the child checked the parent's
  state leaves the child a branch of its own
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:14'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 323000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The narrow race TASK-79.20's fix leaves (round 1). A dead writer's fork-segment PUT can land in object storage after the child has checked the parent's state without it; the child then gets a branch of its own rather than the fork the parent's footer names.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A child created while its parent's dead writer's fork segment is still in flight either sees the fork or refuses, never a branch of its own
- [ ] #2 A simulation test drives the PUT landing after the child's check
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
