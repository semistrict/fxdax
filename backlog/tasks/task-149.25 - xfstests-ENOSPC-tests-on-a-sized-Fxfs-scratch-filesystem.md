---
id: TASK-149.25
title: xfstests' ENOSPC tests on a sized Fxfs scratch filesystem
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 08:09'
updated_date: '2026-11-08 15:38'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
generic/269 crawls: GCE r941 (enospc group, xfstests-fxfs-guest) hit its 150m limit in generic/269; r942 (269 alone, loglevel=7) shows dd and fsstress blocked >10s in fxfs_vfs_write_begin -> sync_filesystem -> fxfs_vfs_sync_fs -> fxfs_sync -> fxfs_rt_park. Cause: every write that finds the filesystem full syncs it (fxfs_vfs_make_room, guest/linux/fs/fxfs/super.c:1635; callers super.c write_begin and page_mkwrite, dax.c's write and fault paths), so 128 writers at ENOSPC each wait behind the others' syncs. Fix: writes share syncs, one begun after a write found no room serving it (room_syncs_begun/done under room_lock), counted in /proc/fs/fxfs/<tag> as vfs_room_asked and vfs_room_syncs. Guest check writers_on_a_full_filesystem_share_syncs (32 writers, 2 writes each, on a full filesystem): red on GCE r943 (a sync each, unserialized-by-count build: 33 writers queued on room_lock, killed past its 10s budget) and r944 (64 syncs for 64 asked, attributes.rs assert).

GCE r941 runs 20261108T082313Z and 20261108T085230Z (tree 005e4e047-dirty, the sized scratch devices and the shared room syncs; collected 2026-11-08 after their waiter was lost, host stopped): xfstests-fxfs-guest and -dax each failed only on tests not in KNOWN_FAILURES, the same on both mounts: generic/015 and generic/274 in the first, generic/211 and generic/679 in the second (17 of 664 failed in all, the rest known). Not yet root-caused (step 3). Checks in the same runs: writers_on_a_full_filesystem_share_syncs failed once (082313), xattrs_bounded twice (085230, TASK-263's slow host durability).
<!-- SECTION:NOTES:END -->
