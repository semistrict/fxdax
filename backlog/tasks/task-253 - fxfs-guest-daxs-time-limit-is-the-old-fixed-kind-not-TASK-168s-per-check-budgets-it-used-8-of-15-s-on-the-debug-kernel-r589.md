---
id: TASK-253
title: >-
  fxfs-guest-dax's time limit is the old fixed kind, not TASK-168's per-check
  budgets: it used 8 of 15 s on the debug kernel (r589)
status: To Do
assignee: []
created_date: '2026-11-08 10:17'
labels:
  - task
dependencies: []
priority: low
ordinal: 686000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Recorded only in TODO.md.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 fxfs-guest-dax's limit comes from TASK-168's budgets
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
