---
id: TASK-91.6
title: >-
  Explain the FAULT errors: 16,012 of 16,822 FAULT requests counted as errors
  under pgbench
status: To Do
assignee: []
created_date: '2026-10-22 18:22'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: medium
ordinal: 271000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd's counters in the sandbox team's matrix run counted 16,012 of 16,822 FAULT requests as errors. Whether that is an expected outcome counted as an error (such as a fault on a sealed extent answered by a remap) or a real failure is unknown.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each FAULT outcome counted as an error is either a real failure, fixed, or recounted as a normal outcome, with the reason in the commit
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
