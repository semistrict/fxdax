---
id: TASK-280
title: >-
  fxfs-guest-suspend-remounted-read-only: the remount read-only fails EBUSY in
  the source VM, before any snapshot
status: To Do
assignee: []
created_date: '2026-11-09 07:16'
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
