---
id: TASK-50.1
title: 'Run under Firecracker''s real seccomp filter and the jailer, as non-root'
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 14:27'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: high
ordinal: 150000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Our tests build Firecracker for the host's glibc target, for which Firecracker has no seccomp filter and allows every syscall; production builds it for musl in release, with the real filter, and the jailer drops each VM to an unprivileged uid. That filter kills the VM on the extent device's thread creation (when the guest activates it, and when a restored VM resumes), on the userfaultfd syscall, and on every UFFDIO_* ioctl the DAX window makes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Firecracker's filters allow the calls the extent device and DAX window make, each only on the threads that make it
- [x] #2 The guest tests run on a musl release build of Firecracker under the jailer, as a non-root user, and pass
- [x] #3 No thread of the device starts before the filter is installed, shown by a test or by construction
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20 (commits 5fa2f5e, vendor/firecracker 124adbc3e): every thread of the extent device and of fxdax in the VMM starts from a spawner under the VMM's filter (AC3 by construction); activation and reset run there too; the VMM filter allows what they do, narrowly (AC1: the log filter's run refused nothing, 26/26 in the jailer as uid 64000). Under the real filter in the jailer: 24/26; fxfs-guest-snapshot-dax (a restored VM's output lost to the runner) and fxfs-guest-migrate-dax (a guest's vsock connect after restore: ENOTCONN) fail in the jail, intermittently (migrate failed once under the log filter too; snapshot passed there), no call refused: AC2 left.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Firecracker built as production builds it (musl, release, its seccomp filter compiled in) runs every guest test in its jailer as uid 64000, with vm.unprivileged_userfaultfd=0: 26/26 on GCE (scripts/test-e2e.sh FXDAX_FIRECRACKER_SECCOMP=real FXDAX_FIRECRACKER_JAIL=1). The extent device's and fxdax's threads start from a spawner under the VMM's filter, activation and reset run there (no thread before its filter, by construction), and the VMM filter allows what they do, narrowly (vendor/firecracker 124adbc3e; 5fa2f5e). The jail's restore failures were a guest test race (TASK-50.1.1/.2).
<!-- SECTION:FINAL_SUMMARY:END -->
