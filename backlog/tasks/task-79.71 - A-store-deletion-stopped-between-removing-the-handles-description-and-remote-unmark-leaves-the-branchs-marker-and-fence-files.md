---
id: TASK-79.71
title: >-
  A store deletion stopped between removing the handle's description and
  remote::unmark leaves the branch's marker and fence files
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
ordinal: 322000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.20 (round 1). A deletion of a host store that stops (crash or error) after the handle's description went but before remote::unmark leaves that branch's marker and fence files in the store's directory until the directory itself goes, so a later attach or listing can see a branch that is being deleted as marked.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A deletion resumed after stopping at any step removes the marker and fence files
- [ ] #2 A test stops a deletion between those steps and checks nothing of the branch is left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
