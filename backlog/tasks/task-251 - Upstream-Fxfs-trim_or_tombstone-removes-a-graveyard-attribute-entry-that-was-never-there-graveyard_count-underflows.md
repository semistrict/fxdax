---
id: TASK-251
title: >-
  Upstream Fxfs trim_or_tombstone removes a graveyard attribute entry that was
  never there: graveyard_count underflows
status: To Do
assignee: []
created_date: '2026-11-08 10:17'
labels:
  - bug
dependencies: []
priority: medium
ordinal: 684000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-160.20 ('found on the way, not fixed'), never filed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test shows the underflow before the fix and none after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
