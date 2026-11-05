---
id: TASK-185
title: fxfs test_trim fails fsck once under heavy load
status: To Do
assignee: []
created_date: '2026-11-05 15:19'
labels:
  - fxfs
  - flaky
dependencies: []
priority: low
type: bug
ordinal: 640000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Seen once on macOS (collector-fixes, 2026-11-05, load average 53, in FXDAX_TEST_PARTS='workspace shuttle' scripts/test.sh): fxfs object_store::data_object_handle::tests::test_trim failed after 70 s: 'fsck_with_options failed: Fsck encountered 1 errors, 0 warnings'. Alone it passes 3 of 3 (8 s). fxfs doesn't depend on extent_remote, which the branch changed. Find the fsck error (run under stress with load, keep fsck's output) and whether trim races something.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 #1 The fsck error test_trim hit is named and reproduced
- [ ] #2 #2 Fixed, with a test red before
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
