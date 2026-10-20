---
id: TASK-50.1.2
title: >-
  Jail: a guest's vsock connection after restore fails with ENOTCONN
  (fxfs-guest-migrate-dax)
status: Done
assignee: []
created_date: '2026-10-20 12:12'
updated_date: '2026-10-20 14:27'
labels: []
dependencies: []
parent_task_id: TASK-50.1
priority: high
type: bug
ordinal: 158000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
In the jailer (GCE runs jail6 under the log filter, real4 under the real filter; passed in jail5 and jail7), fxfs-guest-migrate-dax's restored guest panics at test_fxfs_guest_snapshot.rs:412: connect() to the host after restore fails with ENOTCONN (os error 107). The restored Firecracker's vsock backend connects to the host's Unix socket uds_P, bound by the test in its directory, which the jail binds at the same path; the moved VM's store is in elsewhere/ under the test's directory, served by a daemon the test starts.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Reproduced by looping the case in the jailer
- [x] #2 Cause found and fixed; the case passes 20 rounds in a row in the jailer under the real filter
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The restore resets a guest's vsock connections by an event its kernel may take late, after the test's guest had reconnected (the jail's slower start widened the window): the new connection was reset (ENOTCONN on read, ECONNRESET on connect). The guest now retries on that reset, the host answers every connection (6147f4d). On GCE in the jailer under the real filter: migrate, snapshot, suspend 30 rounds each; whole suite 26/26.
<!-- SECTION:FINAL_SUMMARY:END -->
