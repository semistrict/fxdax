---
id: TASK-153.16
title: 'To try: a panic under a store lock leaves the store usable'
status: To Do
assignee: []
created_date: '2026-11-02 01:59'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 564000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-104, TASK-105 and TASK-150.20 are one pattern: a panic with a store lock held poisons it, and every later request of that guest dies.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decided: what a poisoned store does, and a test of it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
