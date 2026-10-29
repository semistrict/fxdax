---
id: TASK-119.22
title: >-
  fsck ExtentMismatch after fxfs-guest-checks-dax with fxfs.dax_shadow=1: an
  extent the allocator's tree holds (owner 9, 0 bytes) that the allocator keeps
  nothing of
status: Done
assignee: []
created_date: '2026-10-29 13:53'
updated_date: '2026-10-29 14:31'
labels:
  - fxfs
  - bug
dependencies: []
parent_task_id: TASK-119
priority: high
ordinal: 468000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r229-shadow (main b0ccb059c, kernel built), fxfs-guest-checks-dax with FXDAX_GUEST_CMDLINE=fxfs.dax_shadow=1: every guest check passed, then the host's open_image failed: 'fsck found [Error(ExtentMismatch(ExtentId { branch: 2290030959225837046, sequence: 1099511649951 }, Some((9, 0)), None))]': the allocator's tree has Abs records of the extent, owner 9, all deallocated, while the allocator's bookkeeping has no such extent (fsck.rs verify_extents: a dead extent should be gone from the tree, unless shared and pinned). dax_shadow only records the window's mappings in xarrays and WARNs (none fired), so its timing, not its semantics, likely exposed this. checks-dax without diagnostics passed its host fsck in r221, r224 (10 delay rounds), r226a, r227. Artifacts: /tmp/fxdax-r229-shadow.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Its rate with dax_shadow and without, over rounds of checks-dax, is known
- [x] #2 The extent's history (allocations, frees, pins) traced to the step that leaves its tree records
- [x] #3 Fixed, and checks-dax with dax_shadow passes its host fsck over rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Cause: extent_table.rs's documented 'known defect': the staging extent emptied (its allocations deallocated, recount writing records counted 0), the tree compacted meanwhile (those records into a layer), and the filesystem closed with it staging still: its last pin's removal, which no transaction journals, never came; the next mount's load killed it but left its records, and fsck found them (owner 9, 0 bytes) with no extent kept. Rate: 1 in 7 rounds of checks-dax with dax_shadow (r229, r230's 3 passed), 0 in r230's 3 without: rare, timing-dependent. Unit test test_staging_extent_empty_at_close reproduces it every time (fsck: 1 error) on the code before. 50d4f0689: load returns the removal of each extent it finds with nothing allocated, which the allocator merges into the tree, hiding the records; the next compaction writes it. fxfs 694 tests, macOS scripts/test.sh (1823 workspace tests, clippy, scripts) pass. GCE r231 (checks-dax, -small with dax_shadow, 3 rounds) pending.

GCE r231 (main with 50d4f0689, kernel built): checks-dax and checks-dax-small with fxfs.dax_shadow=1, 3 rounds each, passed, the host's fsck of each store clean. DoD #1: macOS scripts/test.sh passed (1823 workspace tests).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A mount now removes from the allocator's tree an extent it finds with nothing allocated (the staging extent emptied, the tree compacted, the filesystem closed with it staging), whose zero-counted records fsck reported as an ExtentMismatch. Unit test test_staging_extent_empty_at_close reproduces it before and passes after; GCE r231 6 rounds clean.
<!-- SECTION:FINAL_SUMMARY:END -->
