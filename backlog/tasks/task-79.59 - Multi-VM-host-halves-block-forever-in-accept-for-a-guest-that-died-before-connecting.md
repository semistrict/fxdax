---
id: TASK-79.59
title: >-
  Multi-VM host halves block forever in accept() for a guest that died before
  connecting
status: In Progress
assignee:
  - '@review-session'
created_date: '2026-10-21 21:00'
updated_date: '2026-10-23 16:10'
labels:
  - review-2026-10-21
dependencies: []
references:
  - docs/reviews/2026-10-21-main.md
parent_task_id: TASK-79
priority: low
type: bug
ordinal: 247000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A guest panic resets the VM and Firecracker exits 0, while the host waits in a blocking accept() until the runner's TIMEOUT, and the panic message is only in the console file.

Where: guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:178, guest/tests/test_cases/src/test_fxfs_guest_follow_torture.rs:258, guest/tests/test_cases/src/test_fxfs_guest_follow.rs:124, guest/tests/test_cases/src/test_fxfs_guest_live_fork.rs:77, guest/tests/test_cases/src/test_fxfs_guest_snapshot.rs:206, guest/tests/guest-agent/src/main.rs:26.

Found by the 2026-10-21 review of main at 6a0a3f8 (severity low): docs/reviews/2026-10-21-main.md, 'Correctness issues', has its evidence and a suggested fix. Related: TASK-42.7, TASK-39.3, TASK-39.1. No task covers the host halves' blocking accept() when the guest dies before connecting.
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
