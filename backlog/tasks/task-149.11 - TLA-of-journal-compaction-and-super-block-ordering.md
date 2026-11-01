---
id: TASK-149.11
title: 'TLA+ of journal, compaction and super-block ordering'
status: To Do
assignee: []
created_date: '2026-11-01 06:13'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 512000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No spec states that a super-block claims only what its journal flushed, which is the invariant the worst bug of 2026-10-31 broke, and a few lines to write.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A spec covers commits, object flushes, device flushes and super-blocks
- [ ] #2 Its invariant fails if offsets are read after the flush
- [ ] #3 It passes on the code's order
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
