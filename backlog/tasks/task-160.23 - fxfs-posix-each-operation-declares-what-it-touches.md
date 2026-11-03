---
id: TASK-160.23
title: 'fxfs-posix: each operation declares what it touches'
status: To Do
assignee: []
created_date: '2026-11-03 13:09'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 594000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every method remembers by hand whether to drain unlinks, whether the failed-batch gate applies (only transaction_with checks it, so clone_range, dax_map_write, dax_update_size, writes, allocate, truncate and xattr writeback bypass it), and whether to borrow metadata space (decided at five sites). Explains TASK-79.21, 26.19.9, 82, 60. Proposal: self.begin(Op::...) returning the guard and options from one table.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One entry per operation
- [ ] #2 The TASK-79.21 cases as tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
