---
id: TASK-250
title: fxfs-posix mutant truncate-gives-up-on-its-tail is caught by no test
status: To Do
assignee: []
created_date: '2026-11-08 10:17'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 683000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found during TASK-160.20/160.25 (recorded in their notes and TODO.md, never filed): the mutant survives on main's base too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test fails with the mutant applied and passes without it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
