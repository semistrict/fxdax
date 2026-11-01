---
id: TASK-153.7
title: 'To try: writers declare what is in flight to the collector'
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 551000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The collector infers in-flight objects from names: TASK-79.3, 79.18, 79.73, 150.1. Format change: do with the others.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decided, with the format changes together
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
