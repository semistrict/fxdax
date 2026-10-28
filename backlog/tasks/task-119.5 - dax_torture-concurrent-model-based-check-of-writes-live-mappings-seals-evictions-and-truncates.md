---
id: TASK-119.5
title: >-
  dax_torture: concurrent model-based check of writes, live mappings, seals,
  evictions and truncates
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 441000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No check runs concurrent writers, long-lived mmap stores, fsync/syncfs from other files, truncate and punch against a model. TASK-112.6 needs a reproducer on main.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Threads on several files: pwrite, stores through mappings kept across ops, fsync/fdatasync, read checks; a sealer, an evictor, and a coordinator doing truncate/punch/extend at barriers
- [ ] #2 Random seed per round, printed; the host compares the stored files with the model after the VM
- [ ] #3 Runs with and without delay injection; reproduces TASK-112.6 on wip/task-112-5
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
