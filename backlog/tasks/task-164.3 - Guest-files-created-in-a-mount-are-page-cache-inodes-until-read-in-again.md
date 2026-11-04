---
id: TASK-164.3
title: 'Guest: files created in a mount are page-cache inodes until read in again'
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 613000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
S_DAX set only at read-in (doc-13).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A created file read in after eviction is DAX unless written since the last seal began
- [ ] #2 Mixed inodes in one superblock pass the DAX and no-DAX suites
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
