---
id: TASK-79.92
title: >-
  Cleaning copies extents into the guest's open segment, so cold data it copied
  is copied again
status: To Do
assignee: []
created_date: '2026-10-23 19:35'
labels:
  - review-2026-10-21
dependencies: []
parent_task_id: TASK-79
priority: medium
type: enhancement
ordinal: 359000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found fixing TASK-79.90 (r5/regress). A pass copies the live extents of mostly-dead segments into the open segment the guest writes, so cold data lands among hot writes, and a later pass finds it in a mostly-dead segment again and recopies it. A segment of cleaning's own for its copies would keep cold data together. The commit makes copies their extents' places only if they are in the open segment (rules().moves_into_the_open_segment), so this needs that rule's reason handled (a footer naming moves into another segment).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cleaning's copies go to a segment of their own, and a commit makes them their extents' places
- [ ] #2 pgbench on GCE shows bytes recopied per pass before and after, and tps no worse
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
