---
id: TASK-79.32
title: >-
  fxfs-guest-daemon-lost boots both VMs from one root image, which building the
  second VM truncates and re-mkfs's
status: To Do
assignee: []
created_date: '2026-10-21 20:58'
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
- [ ] #2 Fixed, and the test passes on macOS and on Linux (GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
