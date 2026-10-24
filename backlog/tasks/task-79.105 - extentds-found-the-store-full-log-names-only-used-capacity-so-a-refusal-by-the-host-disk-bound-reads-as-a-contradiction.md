---
id: TASK-79.105
title: >-
  extentd's 'found the store full' log names only used/capacity, so a refusal by
  the host-disk bound reads as a contradiction
status: To Do
assignee: []
created_date: '2026-10-24 12:30'
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
- [ ] #1 The log, and NoSpace's message, say which bound refused the write and its numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
