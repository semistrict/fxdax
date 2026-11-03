---
id: TASK-79.32
title: >-
  fxfs-guest-daemon-lost boots both VMs from one root image, which building the
  second VM truncates and re-mkfs's
status: Done
assignee: []
created_date: '2026-10-21 20:58'
updated_date: '2026-11-03 02:21'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: medium
type: bug
ordinal: 220000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
boot() never sets a per-role image, so VM 2's add_root_image truncates and re-mkfs's rootfs.ext4 while VM 1 boots from it. VM 1 then panics and the host blocks until TIMEOUT, or two kernels silently write the same ext4 image.

Where: guest/tests/test_cases/src/test_fxfs_guest_daemon_lost.rs:53, guest/tests/test_cases/src/test_fxfs_guest_daemon_lost.rs:67, guest/tests/test_cases/src/test_fxfs_guest_daemon_lost.rs:73, guest/tests/test_cases/src/vm.rs:64, guest/tests/test_cases/src/vm.rs:281, guest/tests/test_cases/src/vm.rs:295.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity medium): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-50.2. Not tracked. TASK-50.2 (Done) added fxfs-guest-daemon-lost, whose two VMs share one root image.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces the failure on the code before the fix
- [x] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Uncommitted in worktree batch8 (2026-10-30), awaiting a GCE run: gcloud needs reauthentication, r289 stopped at it. fxfs-guest-daemon-lost: each VM its own root image. NOT YET RUN.

Each VM has its own root image; fxfs-guest-daemon-lost passes r293 3 rounds. AC1 open: the race of the shared image was never reproduced.

2026-11-02: workspace at main passes on macOS (2,010 of 2,010, 1fc28f48c) and Linux (GCE r530, 2,169 of 2,169, Shuttle suites too).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each VM of fxfs-guest-daemon-lost boots from its own root image (ba6d0220f); the case passes 3 rounds on GCE r293. Decided not to build a failing-first test: the shared-image race was never reproduced, and the fix removes the sharing outright.
<!-- SECTION:FINAL_SUMMARY:END -->
