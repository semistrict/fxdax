---
id: TASK-79.71
title: >-
  A store deletion stopped between removing the handle's description and
  remote::unmark leaves the branch's marker and fence files
status: Done
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 16:07'
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
- [x] #1 A deletion resumed after stopping at any step removes the marker and fence files
- [x] #2 A test stops a deletion between those steps and checks nothing of the branch is left
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A store deletion now removes the branch's marker and fence files, and any .new leftovers, before the handle that names the branch, so a deletion stopped in between is finished by deleting again. A follow-up removes a store's directory with its last branch before the handle too, renaming it out of being a store first, so a stopped removal also finishes. host_store's stopped-deletion test and extent_store's ordering and at-any-step tests pass on macOS and on Linux (GCE). One case is left: a deletion stopped by the old code, on a host that ran it, still leaves its files until the store's directory goes.
<!-- SECTION:FINAL_SUMMARY:END -->
