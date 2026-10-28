---
id: TASK-119.6
title: >-
  crash-dax in the default set, crashing at a random point, with overwrites of
  synced data
status: To Do
assignee: []
created_date: '2026-10-28 19:27'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 442000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
crash-dax crashes at one fixed point, only writes new files with write(2), never overwrites synced data, and is not in the default set.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 In the default e2e and GCE sets
- [ ] #2 Random crash point; workload includes overwrites of synced ranges, mmap stores with msync, truncate
- [ ] #3 After the crash every fsynced state survives; every unsynced byte is old or new
- [ ] #4 A variant kills the VMM with SIGKILL
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
