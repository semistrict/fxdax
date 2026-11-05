---
id: TASK-187
title: >-
  fxfs-guest-crash-stray-dax fails on main: a stray DAX access no longer stops
  the VM with 159
status: To Do
assignee: []
created_date: '2026-11-05 17:15'
labels:
  - bug
  - dax
dependencies: []
priority: high
ordinal: 639000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-165.6's GCE runs and reproduced on main at 75bd872ba (GCE r623, round 2 of 20). The case (TASK-77 AC3) has the guest load or store at a DAX window offset nothing maps (fxfs.dax_stray); the window refuses it, the vCPU's KVM_RUN keeps failing with EFAULT, and Firecracker must exit 159 (FcExitCode::AccessRefused). Now either the VM process exits 0 (r620 round 1, r623 round 2: 'the VM process exited with 0, not 159', after 'KVM_RUN kept failing with EFAULT, ~27000 times in 1 s ... 1 private, 1 withheld'), or vCPU 0's KVM_RUN fails with EFAULT a dozen times at a time and then runs on, thousands of times, until the case's 15 s limit (r621 round 1, r622 round 2). In r620 and r623 the refused access was a kworker's (not a vCPU), the offset then restricted to reads. Likely since copy-on-write DAX (TASK-164.1, vendor/firecracker d53645971: writable DAX deleted, private pages, EFAULT handling). Results: /tmp/fxdax-r620..r623 on the machine that ran them; replay: FXDAX_WORKSPACE_TESTS=0 FXDAX_E2E_TEST=fxfs-guest-crash-stray-dax FXDAX_E2E_ROUNDS=20 scripts/test-gce.sh all.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cause named, with the run that shows it
- [ ] #2 fxfs-guest-crash-stray-dax passes 20 of 20 rounds on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
