---
id: TASK-111.5
title: >-
  Gauges: the window's live mapping count, and the VMM's host mapping count
  against its budget
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 419000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The sandbox team sampled /proc/<pid>/maps every 2 s to see these (TASK-111). Cache entries pinned by window mappings against idle are TASK-72.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 extentd /metrics exports each window live mapping count
- [ ] #2 extentd /metrics exports each VMM host mapping count and its budget
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
