---
id: TASK-50.4
title: 'userfaultfd through /dev/userfaultfd in the jail, with the sysctl off'
status: To Do
assignee: []
created_date: '2026-10-19 20:10'
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
- [ ] #1 The DAX window opens userfaultfd through /dev/userfaultfd when the syscall is refused
- [ ] #2 A test runs with vm.unprivileged_userfaultfd=0, the device node in the jail owned by the jail's group, mode 0660
- [ ] #3 The minimum host kernel is stated (README and doc-4)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
