---
id: TASK-169
title: >-
  Firecracker's seccomp filter kills the VM at the first PRIVATE of a mapped
  page: make_private's pread64 is in no thread's filter
status: To Do
assignee: []
created_date: '2026-11-05 05:02'
updated_date: '2026-11-05 05:46'
labels:
  - bug
  - seccomp
dependencies: []
priority: high
ordinal: 623000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by GCE r570 (2026-11-05, TASK-160.4/160.2's run, us-west1-b): fxfs-guest-checks-dax, fxfs-guest-crash-dax and xfstests-fxfs-guest-dax (generic/029) each ended with Firecracker's "Shutting down VM after intercepting a bad syscall (17)" on its 'extentd client' thread: x86-64 syscall 17 is pread64, which crates/fxdax/src/dax/uffd.rs's make_private calls (read_at) to fill a private page from its source arena's page, and which vendor/firecracker/resources/seccomp/x86_64-unknown-linux-musl.json allows no thread (vmm, api, vcpu). Every guest write to a page mapped from an extent sends PRIVATE (TASK-164.9), so under the real filter (FXDAX_FIRECRACKER_SECCOMP=real, scripts/test-e2e.sh's default) DAX writes kill the VM. Host side; the guest is not involved. Blind spot to close: which run of the guest suites under the real filter since TASK-164.1 passed, and why it didn't reach make_private with a source.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 pread64 (or what make_private uses instead) allowed for the thread that makes pages private, on x86-64 and aarch64
- [ ] #2 The DAX guest suites pass on GCE under FXDAX_FIRECRACKER_SECCOMP=real
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
GCE r571 (2026-11-05, FXDAX_FIRECRACKER_SECCOMP=log, the same guest suites): all passed; seccomp-log.txt has one line, "extentd client 17 762": pread64, 762 times, nothing else.
<!-- SECTION:NOTES:END -->
