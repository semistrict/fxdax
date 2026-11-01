---
id: TASK-153.12
title: 'To try: one trust boundary for layer records'
status: To Do
assignee: []
created_date: '2026-11-01 23:09'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 559000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: journal and super-block records are validated, layer records trusted then checked piecemeal: key order, short bitmaps, entries_in, tombstones. Check each item once in take_item with the journal predicate, as upstream TODO fxbug.dev/42178223 asks. It costs a check on the hottest read path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Prototyped, its cost on the read path measured
- [ ] #2 Decided from the numbers
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
