---
id: TASK-119.11
title: >-
  Host-side invariant: a read-only MAP of an open extent while another window
  range maps it writably is logged and fails tests; the changes log dumped on
  failure
status: To Do
assignee: []
created_date: '2026-10-28 19:45'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 449000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Fable: map_open (crates/fxdax/src/extents.rs) is where an evacuation of an unsealed region would show; the changes recorder already logs window changes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Test runs fail on the invariant
- [ ] #2 A failing guest check dumps the host changes for its window offsets
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
