---
id: TASK-254
title: >-
  First-principles review issue #3: the DAX window's state has three owners;
  option of the host kernel's page cache instead
status: To Do
assignee: []
created_date: '2026-11-08 10:18'
labels:
  - task
dependencies: []
priority: medium
ordinal: 687000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
docs/reviews/2026-11-04-first-principles.md, 'recorded for later', never filed. TASK-160.11 touches part of it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A decision recorded in OPEN_QUESTIONS.md, and the work filed if any
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
