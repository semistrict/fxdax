---
id: TASK-85
title: >-
  extent_store: reaping keeps a tracked segment while an own segment with the
  same number is unuploaded, inherited ones included
status: To Do
assignee: []
created_date: '2026-10-22 13:26'
labels:
  - review-2026-10-21
dependencies: []
priority: low
type: bug
ordinal: 259000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found while restructuring the segments (integrate-2, Segments): the reaping filter checks `unuploaded.contains(&number.number)` by number alone, so an inherited segment whose number matches an own segment not yet uploaded is kept too. It leaks the inherited segment's claim until the own one is uploaded.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test with a fork whose own segment shares an inherited one's number shows the inherited one reaped
- [ ] #2 Fixed by matching segment IDs; the extent_store and extent_remote suites pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
