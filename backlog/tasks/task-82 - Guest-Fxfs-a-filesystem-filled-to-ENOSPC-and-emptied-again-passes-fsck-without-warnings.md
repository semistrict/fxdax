---
id: TASK-82
title: >-
  Guest Fxfs: a filesystem filled to ENOSPC and emptied again passes fsck
  without warnings
status: Done
assignee:
  - '@claude'
created_date: '2026-10-22 13:13'
updated_date: '2026-10-27 20:29'
labels: []
dependencies: []
priority: high
ordinal: 256000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The 'enospc' guest check (TASK-60, uncommitted in the main checkout's guest/tests/test_cases/src/test_fxfs_guest_checks.rs) fills a DAX filesystem of 256 MiB to ENOSPC: one file a MiB at a time, 4 KiB files, a file of every other block, then directories, each until ENOSPC. Then it truncates and removes all of it and writes and reads back 8 MiB. Every step passed in the guest on GCE (2026-10-22, run gce-r60-d), but the host's fsck of the image afterwards (Storage::check, fail_on_warning) failed: 'Volume fsck encountered 0 errors, 1 warnings'. Which FsckWarning it is wasn't logged. Likely something an operation that failed with ENOSPC left behind, such as an orphaned object or attribute, or a graveyard record.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The 'enospc' check is registered in fxfs-guest-checks-dax and its image passes Storage::check
- [x] #2 The warning is identified, and the operation that left it fixed, with a test at Fxfs's level (FakeDevice) that fills to ENOSPC and runs fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Storage::check names the fsck issues it found (fsck found [...]).
2. Run the enospc check (DAX) on GCE to learn the warning.
3. Find the operation that leaves it; fix; Fxfs-level test filling to NoSpace then fsck with fail_on_warning.
4. Register enospc in fxfs-guest-checks-dax (done with TASK-80's merge of branch enospc-check).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The enospc guest check is on branch enospc-check (66f6d20b, pushed), off main at bb562d8c: test_fxfs_guest_checks.rs's test_enospc, DAX only, timeout 180 s. Not yet compiled on that branch; merge it with this task's fix and run fxfs-guest-checks-dax.

Storage::check now names what fsck finds (fsck found [...]). The check alone (FXDAX_CHECKS=enospc) no longer warned on main; after the other checks it did: main, fxfs-guest-checks-dax full run: Volume fsck 1 error; this branch before the fix: UnexpectedObjectInGraveyard(4578) without DAX, GraveyardRecordForAbsentObject(9, 91077) with DAX. Cause: fxfs-linux's tombstone_now tombstoned a file with nothing allocated without looking in the graveyard, removing only its own unlink's entry: a truncate that ran out of room before removing its trim's entry left that entry for an object gone. Fix: it removes an entry found there. Also on a full filesystem, DAX's deferred size update and inode attribute writeback reserved metadata space and failed fsync with ENOSPC: they borrow it now, as upstream's update_attributes does. The Fxfs-level test (fsck::tests::test_filled_and_emptied_without_warnings) fills a volume each way and runs a strict fsck: it passes on main too, the bug being in the guest's own unlink path (no FakeDevice harness); the regression test is the enospc guest check after the others, failing on main and passing 2 rounds with and without DAX, then in the default suites.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Identified the fsck issue as a graveyard entry outliving its object (GraveyardRecordForAbsentObject, or UnexpectedObjectInGraveyard while its tombstone is in a layer): the guest's tombstone_now dropped files with nothing allocated without removing a trim's entry a truncate out of room left. Fixed; size and attribute updates on a full filesystem borrow metadata space so fsync doesn't fail; Storage::check names its findings; enospc is registered in fxfs-guest-checks and -dax. Verified on GCE: full checks fail fsck on main, pass 2 rounds with the fix in both modes, and the default guest suites pass.
<!-- SECTION:FINAL_SUMMARY:END -->
