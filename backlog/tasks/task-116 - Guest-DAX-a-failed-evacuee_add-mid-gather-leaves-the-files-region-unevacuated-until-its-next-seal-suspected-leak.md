---
id: TASK-116
title: >-
  Guest DAX: a failed evacuee_add mid-gather leaves the file's region
  unevacuated until its next seal (suspected leak)
status: To Do
assignee: []
created_date: '2026-10-28 12:45'
labels:
  - bug
dependencies: []
ordinal: 433000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28): fxfs_dax_evacuee_add failing mid-gather returns silently with earlier runs held and later regions unevacuated. A file never written again keeps the region; under memory pressure region space leaks. Unverified.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Confirmed or refuted; if confirmed, the region is retried or released without a further seal of that file
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
