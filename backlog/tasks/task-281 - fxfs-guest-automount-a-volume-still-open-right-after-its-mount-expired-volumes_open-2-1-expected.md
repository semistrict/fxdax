---
id: TASK-281
title: >-
  fxfs-guest-automount: a volume still open right after its mount expired
  (volumes_open 2, 1 expected)
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Trace expiry: autofs expire -> automounter umount2 -> kill_sb -> rmdir -> READY. 2. Find what the test can wait on. 3. Fix the test or the code. 4. GCE: 10 release rounds, 1 debug.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause: the test. umount2's umount_tree takes the mount out of the namespace (mountinfo) at once; the mount's last mntput queues cleanup_mnt as task work on the caller (the automounter), which runs deactivate_super -> fxfs_vfs_kill_sb -> fxfs_unmount (block_on, close_volume) before umount2 returns to it. So the test, polling mountinfo, could count volumes while the second expiry's kill_sb was still closing its volume (2, 1 expected). The code keeps the crate's promise: expire answers READY only after umount2 returned, the volume closed, and after removing the mount's directory. Fix (472daf73d): the test waits for /repos to list no entries (the autofs root's readdir is dcache_readdir: no lookup, no automount), the automounter's rmdir being after the close, then asserts both unmounted and volumes_open == before. Verified on GCE n2-standard-4 us-west1-b: release guest kernel 10/10; debug kernel 1/1. Same debug run: Rust workspace 2420/2420; scripts/tests' 3 host-environment failures filed as TASK-294.
<!-- SECTION:NOTES:END -->
