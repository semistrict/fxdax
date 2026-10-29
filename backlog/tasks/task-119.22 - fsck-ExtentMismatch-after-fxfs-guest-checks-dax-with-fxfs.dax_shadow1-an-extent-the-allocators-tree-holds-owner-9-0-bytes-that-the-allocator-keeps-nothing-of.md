---
id: TASK-119.22
title: >-
  fsck ExtentMismatch after fxfs-guest-checks-dax with fxfs.dax_shadow=1: an
  extent the allocator's tree holds (owner 9, 0 bytes) that the allocator keeps
  nothing of
status: To Do
assignee: []
created_date: '2026-10-29 13:53'
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
- [ ] #1 Its rate with dax_shadow and without, over rounds of checks-dax, is known
- [ ] #2 The extent's history (allocations, frees, pins) traced to the step that leaves its tree records
- [ ] #3 Fixed, and checks-dax with dax_shadow passes its host fsck over rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
