---
id: TASK-92.42
title: A flush syncs a commit's files one after another
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-23 03:10'
updated_date: '2026-10-23 03:14'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: medium
type: enhancement
ordinal: 326000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Measured in TASK-79.2's burst bench (round 1): a durable commit sealing 640 extents in files of their own took about 2.5 s, most of it in Syncs::sync, which fsyncs a commit's files one after another before the directories.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A flush syncs a commit's files in parallel, the directories after them, keeping the order durability needs
- [ ] #2 store-bench's burst case shows the durable commit's time before and after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
