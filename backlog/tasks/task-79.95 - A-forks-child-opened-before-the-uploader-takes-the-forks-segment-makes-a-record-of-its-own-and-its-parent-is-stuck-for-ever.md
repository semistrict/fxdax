---
id: TASK-79.95
title: >-
  A fork's child opened before the uploader takes the fork's segment makes a
  record of its own, and its parent is stuck for ever
status: To Do
assignee: []
created_date: '2026-10-24 00:33'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 365000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.82 (branch r8/forkids). If a fork's child is opened and attached after fork_host returns but before the parent's uploader takes the fork's segment, the child finds no record of its own and no fork in its parent's state, and creates a record of its own. The parent's writer then gives the fork's record to that child, finds it another's (Corrupt 'has another origin'), and every writer of the parent fails for ever; the uploader also overwrites the child's marker. Same area as TASK-79.72 and TASK-79.82.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A child attached before its fork's segment is uploaded waits for it or takes the fork's record, never one of its own, pinned by a Gated object-store test
- [ ] #2 The parent's writers go on after such a race
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
