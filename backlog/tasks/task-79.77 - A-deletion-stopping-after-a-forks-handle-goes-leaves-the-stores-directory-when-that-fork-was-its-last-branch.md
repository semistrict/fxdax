---
id: TASK-79.77
title: >-
  A deletion stopping after a fork's handle goes leaves the store's directory
  when that fork was its last branch
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 06:12'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A deletion resumed after stopping at any step removes the store's directory when its last branch goes
- [x] #2 A test stops a deletion at each of those steps and finds nothing of the store left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A deletion of a store's last branch removes the store's directory before the fork's handle, which 7cc77baf (TASK-79.71) fixed, and now syncs the directory it was in first, so the removal is durable before the handle goes. A test stops such a deletion at every real removal step and around the branch's own deletion. Each time it checks the stop was the injected one, deletes again, and finds nothing of the store or the handle left. With the old order it fails at the step the task names. The tests pass on macOS and on Linux (GCE).
<!-- SECTION:FINAL_SUMMARY:END -->
