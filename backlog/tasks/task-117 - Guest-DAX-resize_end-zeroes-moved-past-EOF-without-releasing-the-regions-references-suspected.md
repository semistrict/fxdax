---
id: TASK-117
title: >-
  Guest DAX: resize_end zeroes moved[] past EOF without releasing the regions'
  references (suspected)
status: Done
assignee: []
created_date: '2026-10-28 12:45'
updated_date: '2026-10-31 02:23'
labels:
  - bug
dependencies: []
ordinal: 434000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
From a review (2026-10-28): fxfs_dax_resize_end zeroes moved[] for pages past the new end but leaves the regions' references to a later prune that may never run (see the relocated/prune task). Unverified.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Confirmed or refuted with a truncate-after-write test checking region release
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. confirmed by reading: resize_end cleared moved[] with no prune after. Fixed: it prunes. Check regions_truncated NOT YET RUN; needs its mutant run.

Mutant GCE r297: regions_truncated passed without the prune: the next write to the range prunes (fxfs_dax_prune from the move path), so a loop over one file shows nothing. Rewritten: 64 files, one truncate each, no later write. r298 the fix, r299 the mutant.

GCE r298 the fix, regions_truncated 3 rounds; r299 the mutant fails: 65 regions more after truncating 64 files, 0 pruned.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Confirmed and fixed: resize_end prunes. regions_truncated, rewritten for many ranges, as a later write to the same range prunes anyway: r298 3 rounds; without the prune 65 regions more after 64 files, 0 pruned (r299).
<!-- SECTION:FINAL_SUMMARY:END -->
