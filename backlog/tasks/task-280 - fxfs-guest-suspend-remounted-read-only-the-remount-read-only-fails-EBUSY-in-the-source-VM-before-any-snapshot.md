---
id: TASK-280
title: >-
  fxfs-guest-suspend-remounted-read-only: the remount read-only fails EBUSY in
  the source VM, before any snapshot
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 07:16'
updated_date: '2026-11-09 09:01'
labels:
  - bug
  - guest-tests
dependencies: []
priority: medium
ordinal: 704000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run t277d (TASK-277; n2-standard-4 us-west1-b, fxdax 2adb9ccd6, Firecracker 14dc26674, release guest kernel), round 1 of 3: the source guest panicked at test_fxfs_guest_snapshot.rs's mount_volume, 'mount default (0x21): Resource busy (os error 16)' (MS_REMOUNT|MS_RDONLY of the default volume), then the VM exited before its guest connected. No snapshot or vsock was involved yet. Suspect: the pieces file stays mapped shared and writable (map_pieces, TASK-191's stores), and the VFS refuses a read-only remount while a file is open for writing; if so the variant fails every time it runs after TASK-191, so check whether it was in any recent suite. Artifacts: the run's results/e2e/fxfs-guest-suspend-remounted-read-only/round-1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Root cause of the EBUSY found and fixed
- [ ] #2 fxfs-guest-suspend-remounted-read-only passes 10 rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Confirm EBUSY's source in the VFS. 2. Compare with ext4. 3. Find when the case broke. 4. Fix the test. 5. GCE: 10 release rounds, 1 debug.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause: the test. map_pieces opens the pieces file O_RDWR and keeps it mapped MAP_SHARED|PROT_WRITE; the vma holds the file, which do_dentry_open marked FMODE_WRITER (file_get_write_access -> mnt_get_write_access), so reconfigure_super's sb_prepare_remount_readonly -> mnt_hold_writers returns EBUSY. Generic VFS, before fxfs_vfs_reconfigure runs: ext4 refuses the same remount the same way, so guest Fxfs is right. History: the case passed when it landed (6e93f8a47, GCE r762); 6e3b379a6 (TASK-92.14/TASK-191's test, rebased onto main after it) made the pieces mapping writable and ran only -snapshot-dax/-suspend-dax/-migrate, so the variant has failed every run since. Fix (086fd0e7e): ReadOnly::Remounted unmaps the writable mapping and maps the file again read-only (map takes a length) before the remount, reading it all, as an integrator would. Verified on GCE n2-standard-4 us-west1-b: release guest kernel 10/10; debug kernel 1/1.
<!-- SECTION:NOTES:END -->
