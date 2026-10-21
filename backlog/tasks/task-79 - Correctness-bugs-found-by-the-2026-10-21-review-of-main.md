---
id: TASK-79
title: Correctness bugs found by the 2026-10-21 review of main
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
priority: high
type: bug
ordinal: 188000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A multi-agent review of fxdax's own code at 6a0a3f8 (docs/reviews/2026-10-21-main.md) confirmed 65 correctness bugs, each checked by two independent skeptics and a tie-breaker where they split: 15 high (data loss or corruption, host crash, security boundary, hang), 17 medium and 33 low. Each is a subtask; the report has the evidence and a suggested fix for each, under 'Correctness issues'.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every subtask is Done, or closed with evidence that the finding was wrong
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
