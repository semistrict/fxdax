---
id: TASK-79.85
title: >-
  Branches in object storage that no handle names, made before TASK-79.78, are
  never reclaimed
status: To Do
assignee: []
created_date: '2026-10-23 09:16'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: low
type: task
ordinal: 344000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-79.78 (round 2c) stops a failed fork from leaving a branch no handle names: a deletion that stops partway leaves the fork's handle, and deleting it lets the collector free what the fork inherited. Orphans made before it (a live record, no handle on any host) stay: the collector cannot tell a branch no store holds from one held idle on some host without a liveness protocol, so it keeps what they inherited for ever.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A way exists to find and delete branches no host holds (a liveness record, or an operator command that lists records against hosts' handles)
- [ ] #2 Deleting one lets the collector free what it inherited, pinned by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
