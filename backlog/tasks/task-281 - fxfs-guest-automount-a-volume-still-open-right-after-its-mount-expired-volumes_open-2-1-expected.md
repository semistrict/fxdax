---
id: TASK-281
title: >-
  fxfs-guest-automount: a volume still open right after its mount expired
  (volumes_open 2, 1 expected)
status: To Do
assignee: []
created_date: '2026-11-09 07:16'
labels:
  - bug
  - guest-tests
dependencies: []
priority: medium
ordinal: 705000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE run t277d (TASK-277; n2-standard-4 us-west1-b, fxdax 2adb9ccd6, Firecracker 14dc26674, release guest kernel), round 1 of 3: the consumer panicked at test_fxfs_guest_automount.rs:216, assert_eq!(volumes_open(), before) right after wait_for('the mounts to expire'): 2 open, 1 expected. The automounter made no vsock retry (no reset in the console). Suspect a race in the check: the expiry's unmount lands (mounted() is None) before the volume it mounted is closed. Either the test waits for the volume to close, or the automounter's expire returns after the close, which the doc of crates/fxfs-automount promises ('its volume closed'). Artifacts: the run's results/e2e/fxfs-guest-automount/round-1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Root cause found and fixed
- [ ] #2 fxfs-guest-automount passes 10 rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
