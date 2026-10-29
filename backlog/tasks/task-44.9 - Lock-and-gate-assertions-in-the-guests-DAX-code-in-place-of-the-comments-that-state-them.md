---
id: TASK-44.9
title: >-
  Lock and gate assertions in the guest's DAX code in place of the comments that
  state them
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-29 21:01'
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
- [x] #1 Each function whose comment names a lock or the gate asserts it (lockdep_assert_held, or a gate assertion added for it)
- [x] #2 The guest suites pass on a lockdep kernel with them
- [x] #3 Removing a lock a function asserts fails a guest test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
c7dacce4d: the gate's lockdep map (shared from store_begin to store_end, whole while a seal waits), fxfs_device_assert_in_gate, and lockdep_assert_held(_write) where the comments named the gate, the file's DAX semaphore, the regions lock or the invalidate_lock; map_to asserts the gate for writable MAPs alone (a reader's read-only MAP is outside it, contrary to its comment). Debug kernel (lockdep, KCSAN), GCE r80/r81: no assertion or lockdep report in the DAX suites. AC3 left: the hand mutants (TASK-44.19) removing a lock.

AC3: mutant regions_find_unlocked (fxfs_dax_get_region's lookup without the regions lock), run on a debug (lockdep) kernel, as guest-mutants-gce.py now lets a mutant name (its fourth field): GCE r255, killed in round 1 of checks-dax by fxfs_dax_find_region's lockdep_assert_held, 'WARNING ... at fs/fxfs/dax.c:1615 fxfs_dax_find_region', 5.6 s into the run. (A first try, r254, lost its host to its 8 h lifetime.)
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The guest's DAX functions whose comments name a lock or the gate assert it (lockdep, a gate lockdep map), the suites pass on a lockdep kernel with them, and removing a lock an asserting function needs (mutant regions_find_unlocked, on a debug kernel) fails a guest test at the assertion (GCE r255).
<!-- SECTION:FINAL_SUMMARY:END -->
