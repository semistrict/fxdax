---
id: TASK-92.56
title: >-
  Index checkpoints are appended inline under the state lock, costing an ordered
  commit 89-143 ms
status: To Do
assignee: []
created_date: '2026-10-24 07:32'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 376000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found measuring TASK-91.17 (no-DAX pgbench -i on GCE). Three ordered commits of the load spent 89-143 ms in append_record, writing index checkpoints inline while holding the store's state lock, which the guest's writes wait for.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit's index checkpoint is written outside the state lock, or bounded, shown by the load's commit times before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
