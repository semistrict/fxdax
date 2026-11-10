---
id: TASK-308
title: >-
  Firecracker's extent device dies of seccomp (bad syscall 323) when
  /dev/userfaultfd isn't accessible, instead of saying so
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 13:07'
updated_date: '2026-11-10 09:30'
labels:
  - bug
  - vmm
dependencies: []
priority: low
ordinal: 717000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-09, main 027a3926, non-root user; /dev/userfaultfd root:root 0600, vm.unprivileged_userfaultfd=0), fxfs-guest-checks-dax-delay's VM stopped 1 s in with nothing from the guest: firecracker.log 'Shutting down VM after intercepting a bad syscall (323)' in fc_extent_activate. The userfaultfd crate's UffdBuilder::create opens /dev/userfaultfd and, failing (EACCES), falls back to the userfaultfd(2) syscall, which the production seccomp filter forbids. The extent device should open /dev/userfaultfd itself (or build without the syscall fallback) and fail the activation with the error ('/dev/userfaultfd: permission denied'), so a misconfigured host says what it lacks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 With /dev/userfaultfd inaccessible, the VM fails with an error naming /dev/userfaultfd and the permission, not a seccomp kill
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxdax (crates/fxdax/src/dax/uffd.rs): userfaultfds through /dev/userfaultfd alone, no userfaultfd(2) fallback; open_device's error names the device, the errno, the uid/gid and what gives access (group/mode udev rule, ACL, jail node), and says vm.unprivileged_userfaultfd serves no VMM. Public fxdax::userfaultfd_accessible().
2. Firecracker series: one commit on fxdax-v1.16.1-r4: Extent::make, for a device with a DAX window, calls it before opening the store, failing with ExtentError::Userfaultfd(id, err), so InstanceStart fails, not the guest's activation. Retag fxdax-v1.16.1-r5 (or next) after firecracker-series-check.sh passes; vendor/firecracker to it.
3. Tests: fxdax unit test of the error's text; on ubu1, a VM with /dev/userfaultfd hidden (a mount namespace binding a root-only file over it) fails at start with the message.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
The fallback was fxdax's (dax::uffd::open_userfaultfd), not the userfaultfd crate's. Removed: /dev/userfaultfd alone, so the class (a system call the VMM's filter refuses) can't come from here; the error names the device, errno, uid/gid as the host has them (user-namespace maps: the runner's VMMs are root in theirs, which the first try printed), the fix, and that vm.unprivileged_userfaultfd doesn't serve. Firecracker fxdax-v1.16.1-r5 (b968bb10b on r4, branch fxdax-v1.16.1-uffd-device): Extent::make checks it for a device with a window, so InstanceStart fails. Series check from r4 both ways under clippy -D warnings; test_config_space_as_upstream passes without the feature. Unit tests: the error's text; host_id's maps. E2E on ubu1: /dev/userfaultfd hidden from ramon (mount namespace, a root 0600 file bound over it), fxfs-guest-checks-dax-delay: firecracker.log 'The extent device fxdax has a DAX window, which cannot take its guest's faults: /dev/userfaultfd: Permission denied (os error 13): ... (uid 1000, gid 1000) ...', exit 1, no bad syscall. Red: the task's own report (bad syscall 323). fxdax nextest: ubu1 575/576 (imports_read_what_they_made_from_the_hosts_cache is TASK-316's /tmp, passes with TMPDIR on disk; windows_larger_than_the_map_count_read_whole OOM-killed once under others' load, passes alone), macOS 330/330. Decision in OPEN_QUESTIONS.md. TASK-276.6 hadn't tagged a newer release; r5 is on r4.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
No userfaultfd(2) fallback; an extent device with a DAX window fails the VM at start naming /dev/userfaultfd, the errno, the host uid/gid and the fix (Firecracker fxdax-v1.16.1-r5). Verified on ubu1 with the device hidden.
<!-- SECTION:FINAL_SUMMARY:END -->
