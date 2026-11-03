---
id: TASK-149.25
title: xfstests' ENOSPC tests on a sized Fxfs scratch filesystem
status: To Do
assignee: []
created_date: '2026-11-03 08:09'
labels:
  - testing
dependencies: []
parent_task_id: TASK-149
priority: high
ordinal: 570000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
xfstests' enospc group runs 2 of its 22 tests on the guest's Fxfs (generic/103, generic/213; GCE 2026-11-03, TASK-60): 14 are 'not run' as _scratch_mkfs_sized doesn't know fxfs (generic/015, 027, 077, 083, 226, 269, 274, 275, 312, 371, 416, 449, 558, 619), and 4 want a block device (019, 299, 300, 546). These are the tests that fill a filesystem to ENOSPC, the paths TASK-60 says must never panic the guest's kernel. The harness (guest/tests/test_cases/src/test_xfstests.rs) treats fxfs as virtiofs in common/rc, whose mkfs only empties the scratch directory. Make _scratch_mkfs_sized work for fxfs: a scratch device of the size asked for, made by the host, or Fxfs reformatting a scratch volume capped to it, and teach common/rc's _scratch_mkfs_sized the fxfs case.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The enospc group's _scratch_mkfs_sized tests run on xfstests-fxfs-guest and -dax
- [ ] #2 Each passes, or is a known failure with its reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
