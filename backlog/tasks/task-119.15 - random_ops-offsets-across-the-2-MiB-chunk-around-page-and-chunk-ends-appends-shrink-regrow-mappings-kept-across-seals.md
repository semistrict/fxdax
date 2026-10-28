---
id: TASK-119.15
title: >-
  random_ops: offsets across the 2 MiB chunk, around page and chunk ends,
  appends, shrink/regrow, mappings kept across seals
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 453000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra: MAX is 1 MiB, below the DAX chunk; mappings are made and dropped per op.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Ops reach past 2 MiB and keep a mapping across seals; histories saved for replay
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
