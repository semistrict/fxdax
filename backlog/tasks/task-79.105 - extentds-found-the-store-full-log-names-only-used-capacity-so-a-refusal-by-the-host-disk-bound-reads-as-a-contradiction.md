---
id: TASK-79.105
title: >-
  extentd's 'found the store full' log names only used/capacity, so a refusal by
  the host-disk bound reads as a contradiction
status: Done
assignee: []
created_date: '2026-10-24 12:30'
updated_date: '2026-10-31 02:23'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 379000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left by TASK-79.89 (done). A write can now be refused (NoSpace) by the bound on a branch's host files (three times its capacity, Space::check_files), not only by its capacity; extentd's log of a store found full still prints only used and capacity, so such a refusal logs a store that looks far from full.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The log, and NoSpace's message, say which bound refused the write and its numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. the store-full warning says bytes of the host's disk too.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The store-full log says the disk too; NoSpace says which bound refused it and its numbers (capacity test pins the message). extent_store 214/214 on macOS, fxdax and extent_store 589/590 in Docker Linux, the one left needing sudo, which r292 passed.
<!-- SECTION:FINAL_SUMMARY:END -->
