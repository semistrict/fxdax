---
id: TASK-44.9
title: >-
  Lock and gate assertions in the guest's DAX code in place of the comments that
  state them
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 388000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
dax.c, device.c and super.c say in prose what each function holds: 'in the gate', 'with the file's DAX semaphore held for writing', 'under the regions lock'. Nothing checks it, and TASK-99 broke one such rule silently. lockdep_assert_held and an assertion of being in the seal gate turn each into a check that fails at once; Sparse's context annotations (__must_hold) check some statically.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each function whose comment names a lock or the gate asserts it (lockdep_assert_held, or a gate assertion added for it)
- [ ] #2 The guest suites pass on a lockdep kernel with them
- [ ] #3 Removing a lock a function asserts fails a guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
