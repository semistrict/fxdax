---
id: TASK-79.92
title: >-
  Cleaning copies extents into the guest's open segment, so cold data it copied
  is copied again
status: Done
assignee: []
created_date: '2026-10-23 19:35'
updated_date: '2026-11-06 06:50'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Moot after TASK-165.8 (a31f82a98): local cleaning is deleted (Store::clean, clean_behind, cleaning's copies and the moves_into_the_open_segment rule are gone: rg finds none in crates/ at f81174090). Acknowledged segments leave the store for the host's part cache; the bucket's collector, which compacts into objects of its own, is the only cleaner.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Closed as moot: TASK-165.8 deleted local cleaning, whose copies into the open segment this was about; the bucket's collector is the only cleaner.
<!-- SECTION:FINAL_SUMMARY:END -->
