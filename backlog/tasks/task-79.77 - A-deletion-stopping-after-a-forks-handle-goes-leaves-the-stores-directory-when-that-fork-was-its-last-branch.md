---
id: TASK-79.77
title: >-
  A deletion stopping after a fork's handle goes leaves the store's directory
  when that fork was its last branch
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 06:48'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 335000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found reviewing TASK-79.71 (round 2a); same family as TASK-79.20. In crates/extent_store/src/host.rs, a deletion that stops after a fork's handle description goes, or after its directory goes, is then 'finished' by delete_host_after's early return, or fails with ENOENT through host_store::delete; neither reaches the removal of the store directory. When that fork was the store's last branch, the store's directory and everything it shared stay, and no handle names them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A deletion resumed after stopping at any step removes the store's directory when its last branch goes
- [ ] #2 A test stops a deletion at each of those steps and finds nothing of the store left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
