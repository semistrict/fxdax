---
id: TASK-157
title: >-
  Tests that fail under load: forks failing at each step, and windows sharing a
  base
status: To Do
assignee: []
created_date: '2026-11-02 01:59'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 562000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From TASK-150.20 under load: guests_forks_failing_at_each_step_harm_nothing fails 10 of 10 on base and 8 of 10 on head; windows_share_bases_pages 136 of 500 on base, 41 of 500 on head. Neither is that work. Each is a race in the test or in the code: not known which.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each cause found, in the test or the code
- [ ] #2 Each passes 500 runs under load
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
