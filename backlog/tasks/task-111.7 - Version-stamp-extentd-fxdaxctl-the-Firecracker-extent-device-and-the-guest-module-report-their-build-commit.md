---
id: TASK-111.7
title: >-
  Version stamp: extentd, fxdaxctl, the Firecracker extent device and the guest
  module report their build commit
status: To Do
assignee: []
created_date: '2026-10-28 01:25'
updated_date: '2026-10-30 01:37'
labels: []
dependencies: []
parent_task_id: TASK-111
priority: medium
ordinal: 421000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every sandbox result should trace to an exact build (TASK-111); today they record commits by hand.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 extentd /metrics exports its build commit
- [x] #2 fxdaxctl --version prints its build commit
- [x] #3 The VMM reports its build commit to extentd and /metrics shows it per VM
- [ ] #4 The guest module shows its build commit under /proc/fs/fxfs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Host side on main (e04c0daa9): scripts/build-commit.sh, one source (FXDAX_BUILD_COMMIT, else git's commit with -dirty, else the tarball's source-revision.txt, which gce.sh now fills from it, else unknown); crates/fxdax/build.rs stamps fxdax::BUILD_COMMIT. AC1: /metrics fxdax_extentd_build_info{commit} 1. AC2: fxdaxctl --version, test the_version_is_the_build_commit. AC3: the VMM's extentd client (fxdax's, no vendor/firecracker change) sends BUILD (wire kind 28) after OPEN, older daemons refusing it EINVAL, ignored; /metrics fxdax_extentd_vm_build_info{branch,commit} N; Linux test metrics_show_the_daemons_and_its_vm_processes_builds (Docker). Verified: macOS fxdax 239, Docker fxdax 366 of 368 (stacks, grants: Docker only), test_gce and test_build_guest_kernel, upstream-changes --check. AC4 on branch task-111.7-guest (c1311da84, not pushed): FXFS_BUILD_COMMIT into fs/fxfs's make from build-guest-kernel.sh, /proc/fs/fxfs/build 'commit <sha>', fxfs-guest checks it; needs a GCE release run of fxfs-guest and fxfs-guest-dax. In Docker the stamp is 'unknown' (the worktree's git dir isn't mounted): builds_are_stamped_with_their_commit passes there with -e FXDAX_BUILD_COMMIT.
<!-- SECTION:NOTES:END -->
