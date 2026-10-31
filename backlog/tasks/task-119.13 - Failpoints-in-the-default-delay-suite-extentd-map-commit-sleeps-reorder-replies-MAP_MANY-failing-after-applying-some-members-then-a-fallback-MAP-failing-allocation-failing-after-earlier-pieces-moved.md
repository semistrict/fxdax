---
id: TASK-119.13
title: >-
  Failpoints in the default delay suite: extentd map/commit sleeps reorder
  replies; MAP_MANY failing after applying some members, then a fallback MAP
  failing; allocation failing after earlier pieces moved
status: Done
assignee: []
created_date: '2026-10-28 19:45'
updated_date: '2026-10-31 06:53'
labels:
  - testing
dependencies: []
parent_task_id: TASK-119
priority: medium
ordinal: 451000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Astra/Fable: FXDAX_FAILPOINTS exists but no default run uses it; dax.c publishes pieces incrementally (map_extents) and TASK-79.106 has no deterministic test.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Partial-failure paths each reached by a check that verifies contents after
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: fxfs.dax_fail_map_many, dax_fail_fallbacks, dax_fail_moves; checks map_many_failed, move_failed. Found: a write to a chunk with no range moves its blocks first (fxfs_dax_prepare_write), a whole page without its old bytes, so a map failing after left them zeros, with EIO (or ENOMEM from the range's allocation). It is now retried once as a failure after a move (TASK-79.106): mutant moved_write_not_retried fails map_many_failed (r315). A second failure still leaves zeros, the write having failed. The delay suite's failpoints (FXDAX_FAILPOINTS) not turned on.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each partial-failure path has a check that verifies contents after; one real loss of old bytes found and fixed.
<!-- SECTION:FINAL_SUMMARY:END -->
