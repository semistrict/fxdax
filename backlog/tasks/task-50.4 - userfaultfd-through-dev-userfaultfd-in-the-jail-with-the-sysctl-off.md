---
id: TASK-50.4
title: 'userfaultfd through /dev/userfaultfd in the jail, with the sysctl off'
status: Done
assignee: []
created_date: '2026-10-19 20:10'
updated_date: '2026-10-20 11:55'
labels: []
dependencies: []
parent_task_id: TASK-50
priority: medium
ordinal: 153000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Production will not set vm.unprivileged_userfaultfd=1: it is node-wide and gives every process a well-known kernel-exploit primitive. Firecracker drops every privilege in its jail, so the DAX window's userfaultfd must come from /dev/userfaultfd made inside the jail.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The DAX window opens userfaultfd through /dev/userfaultfd when the syscall is refused
- [x] #2 A test runs with vm.unprivileged_userfaultfd=0, the device node in the jail owned by the jail's group, mode 0660
- [x] #3 The minimum host kernel is stated (README and doc-4)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-20: AC2 as Firecracker's jailer makes the node: /dev/userfaultfd in the jail owned by the VMM's uid and gid, mode 0600 (not group-owned 0660, which a jail made by hand may use instead; README says both). scripts/test-e2e.sh FXDAX_FIRECRACKER_JAIL=1 sets vm.unprivileged_userfaultfd=0 and runs every guest test so: 26 of 26 under the seccomp log filter (GCE). Minimum kernel 6.1 in README and doc-4 (commit 0b57ffa).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The DAX window opens userfaultfd through /dev/userfaultfd (already so); the jail runs (scripts/test-e2e.sh FXDAX_FIRECRACKER_JAIL=1) run every guest test with vm.unprivileged_userfaultfd=0 and the jailer's /dev/userfaultfd, 26/26 under the log filter on GCE; README and doc-4 state Linux 6.1+.
<!-- SECTION:FINAL_SUMMARY:END -->
