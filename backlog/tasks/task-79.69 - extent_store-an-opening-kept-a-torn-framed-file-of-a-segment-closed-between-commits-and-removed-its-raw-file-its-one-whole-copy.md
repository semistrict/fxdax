---
id: TASK-79.69
title: >-
  extent_store: an opening kept a torn framed file of a segment closed between
  commits and removed its raw file, its one whole copy
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: high
type: bug
ordinal: 320000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while fixing TASK-79.33 (round 1). A segment closed between commits names in its footer commits that may be durable before its framed file is written and synced. A crash or failure between the write and the sync left a framed file whose trailer, table and footer were whole, but a data part torn or not durable. The branch's next opening kept it and removed the raw file, and reads failed their CRCs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An opening keeps a framed file only if its data is whole, and syncs it before the raw file goes; otherwise it frames the raw file anew
- [ ] #2 A crash test with a torn data part opens and reads every durable commit's data
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
