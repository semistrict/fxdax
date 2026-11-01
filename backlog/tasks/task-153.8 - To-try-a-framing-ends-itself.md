---
id: TASK-153.8
title: 'To try: a framing ends itself'
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 552000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Framing end is driven from outside, across closings, Pending.closings, framings and Phase::Framing: TASK-41.1, 79.69, 79.99, 92.16, 150.2. Flushes would only wait for it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decided, with a prototype
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
