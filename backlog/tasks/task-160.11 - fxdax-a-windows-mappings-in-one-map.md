---
id: TASK-160.11
title: 'fxdax: a window''s mappings in one map'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 582000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Dax<K> copies WindowState's interval map and by-source index under another lock, encoding the source page differently; DEFERRED is per-thread and shared by every window's RemoteStage unchecked. Explains TASK-79.10. Proposal: the window's Mapping carries K; deferral lives in the RemoteStage.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One map per window
- [ ] #2 dax shuttle tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
