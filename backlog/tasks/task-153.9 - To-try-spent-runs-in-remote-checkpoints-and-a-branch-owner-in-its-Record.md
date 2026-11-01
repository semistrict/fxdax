---
id: TASK-153.9
title: 'To try: spent runs in remote checkpoints, and a branch owner in its Record'
status: To Do
assignee: []
created_date: '2026-11-01 22:53'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 553000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Written-once state lives on one host while the bucket is the truth (TASK-79.6, 150.6), and branches have no owner (TASK-150.4). Both format changes; TASK-150.6 needs the first. Bundle with the collector change in one format bump.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A format change designed for them together
- [ ] #2 TASK-150.6 fixed by it, its test red first
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
