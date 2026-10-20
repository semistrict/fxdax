---
id: TASK-50.1
title: 'Run under Firecracker''s real seccomp filter and the jailer, as non-root'
status: In Progress
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-19 20:32'
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
- [ ] #1 Firecracker's filters allow the calls the extent device and DAX window make, each only on the threads that make it
- [ ] #2 The guest tests run on a musl release build of Firecracker under the jailer, as a non-root user, and pass
- [ ] #3 No thread of the device starts before the filter is installed, shown by a test or by construction
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
