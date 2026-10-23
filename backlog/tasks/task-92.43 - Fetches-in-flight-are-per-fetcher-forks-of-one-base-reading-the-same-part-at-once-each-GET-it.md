---
id: TASK-92.43
title: >-
  Fetches in flight are per fetcher: forks of one base reading the same part at
  once each GET it
status: To Do
assignee: []
created_date: '2026-10-23 03:10'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-92
priority: low
type: enhancement
ordinal: 327000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-92.21 (round 1) made the fetched-parts memory host-wide, so forks of one base share parts once fetched. GETs in flight (fetch::Parts) are still per fetcher: forks booting from one base that read the same part at the same moment each send a GET, and the shared memory dedupes only once the first lands.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Concurrent reads of one object part from several stores' fetchers send one GET
- [ ] #2 A test counts the GETs of N fetchers reading one part at once
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
