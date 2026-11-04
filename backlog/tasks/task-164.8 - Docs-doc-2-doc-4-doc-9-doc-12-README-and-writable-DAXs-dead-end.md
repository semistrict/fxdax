---
id: TASK-164.8
title: 'Docs: doc-2, doc-4, doc-9, doc-12, README, and writable DAX''s dead end'
status: To Do
assignee: []
created_date: '2026-11-04 07:58'
labels: []
dependencies: []
parent_task_id: TASK-164
priority: high
type: task
ordinal: 618000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-13 stage 4's docs.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each says what copy-on-write DAX does, nothing of seals moving pages
- [ ] #2 dead-ends/ holds writable DAX, with why
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
