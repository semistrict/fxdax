---
id: TASK-149.25
title: xfstests' ENOSPC tests on a sized Fxfs scratch filesystem
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 08:09'
updated_date: '2026-11-07 23:59'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A scratch device per size the enospc tests ask _scratch_mkfs_sized for (128, 240, 256, 330, 512 MiB, 1, 2, 5 GiB), its extent store as large and empty, so its first mount formats an Fxfs of that size (device.rs: block_count = capacity / block size; fs.rs: an empty store is formatted); its DAX window as large under -dax.
2. The image's common/rc, after xfstests' own: _try_scratch_mkfs_sized for fxfs points SCRATCH_DEV at the device of that size, mounted (else _notrun) and emptied; _scratch_unmount by the mount point, as check unmounts whichever device a test left mounted. Host tests: the snippet written byte for byte, and its behaviour under bash with xfstests' functions stubbed.
3. GCE: the enospc group on xfstests-fxfs-guest and -dax; each failure root-caused (file:line), fixed with a test that fails first, or filed with evidence; KNOWN_FAILURES for what stays, with reasons.
4. generic/quick on both, for what the new devices change.
<!-- SECTION:PLAN:END -->
