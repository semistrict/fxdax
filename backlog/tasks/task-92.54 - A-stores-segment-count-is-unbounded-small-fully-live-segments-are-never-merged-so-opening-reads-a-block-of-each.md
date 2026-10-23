---
id: TASK-92.54
title: >-
  A store's segment count is unbounded: small fully-live segments are never
  merged, so opening reads a block of each
status: To Do
assignee: []
created_date: '2026-10-23 23:09'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 364000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left by TASK-92.16 (accepted by the user 2026-10-23). Commits no longer scan every segment, but opening a store still reads one block of each live closed segment: 12.9 MB in 4.5 ms at 2,000 segments, 90 MB in 35 ms at 20,000 (store-bench segments). close_now has no minimum size, and cleaning takes only segments under its live fraction, so small fully-live segments are never merged and their count grows with a store's age.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A store's closed segment count stays within a bound, by merging small fully-live segments or a minimum close size
- [ ] #2 store-bench segments shows opening's time flat in the store's age
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
