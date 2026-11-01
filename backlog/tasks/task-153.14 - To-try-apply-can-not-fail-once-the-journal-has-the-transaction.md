---
id: TASK-153.14
title: 'To try: apply can not fail once the journal has the transaction'
status: To Do
assignee: []
created_date: '2026-11-01 23:09'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: low
type: task
ordinal: 561000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.9: a live transaction that cannot apply can only stop the journal, so a panic becomes a dead filesystem (journal.rs:2211).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Decided, with what a failed apply should do
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
