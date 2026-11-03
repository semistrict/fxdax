---
id: TASK-160.19
title: 'extent_store: a store''s open, closed and failed state as one value'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 590000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
failed and closed are booleans; a store fails three ways; close_between_commits repeats still_writable differently; 16 lock sites each decide what failed or closed means. Explains TASK-83, 79.99, 153.16. Proposal: enum Life read through readable()/writable() guards, re-checking after waiting structural. After TASK-153.8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One life value behind guards
- [ ] #2 recheck_after_waiting can't be broken by a caller
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
