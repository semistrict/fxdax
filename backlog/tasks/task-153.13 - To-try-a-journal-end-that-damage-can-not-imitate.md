---
id: TASK-153.13
title: 'To try: a journal end that damage can not imitate'
status: To Do
assignee: []
created_date: '2026-11-01 23:09'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: medium
type: task
ordinal: 560000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: a bad block reads as the end of the journal, so a flip mid-journal silently loses synced transactions: 25 to 32 of 200 cases on a crashed image. Needs a durable end marker outside the journal.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Designed, with what it costs a commit
- [ ] #2 Decided
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
