---
id: TASK-82
title: >-
  Guest Fxfs: a filesystem filled to ENOSPC and emptied again passes fsck
  without warnings
status: To Do
assignee: []
created_date: '2026-10-22 13:13'
updated_date: '2026-10-24 00:54'
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
- [ ] #1 The 'enospc' check is registered in fxfs-guest-checks-dax and its image passes Storage::check
- [ ] #2 The warning is identified, and the operation that left it fixed, with a test at Fxfs's level (FakeDevice) that fills to ENOSPC and runs fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The enospc guest check is on branch enospc-check (66f6d20b, pushed), off main at bb562d8c: test_fxfs_guest_checks.rs's test_enospc, DAX only, timeout 180 s. Not yet compiled on that branch; merge it with this task's fix and run fxfs-guest-checks-dax.
<!-- SECTION:NOTES:END -->
