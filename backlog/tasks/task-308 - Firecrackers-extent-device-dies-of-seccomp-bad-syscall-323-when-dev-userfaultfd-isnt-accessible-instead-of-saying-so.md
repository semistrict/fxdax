---
id: TASK-308
title: >-
  Firecracker's extent device dies of seccomp (bad syscall 323) when
  /dev/userfaultfd isn't accessible, instead of saying so
status: To Do
assignee: []
created_date: '2026-11-09 13:07'
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
- [ ] #1 With /dev/userfaultfd inaccessible, the VM fails with an error naming /dev/userfaultfd and the permission, not a seccomp kill
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
