---
id: TASK-44.8
title: >-
  Sanitized guest kernel: KCSAN, lockdep, KASAN, KFENCE and atomic-sleep checks
  in the guest suites
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-24 23:57'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 387000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-99 was a guest data race: fxfs_dax_region_open read region->mapped under another lock than the MAP in flight held, and the guest and host disagreed on a page's writability until the VM died. KCSAN and lockdep find such races and lock-order bugs as they happen, in any run, where stress only finds them by luck. build-guest-kernel.sh has FXDAX_GUEST_KERNEL_LOCK_STAT; nothing builds the guest with the kernel's sanitizers.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 build-guest-kernel.sh builds a sanitized guest kernel (KCSAN, PROVE_LOCKING, KASAN, KFENCE, DEBUG_ATOMIC_SLEEP) on a switch
- [ ] #2 The guest suites run on it on GCE, and every report it makes is fixed or filed
- [ ] #3 TASK-99's race, put back by hand, is reported by it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Committed: the runner fails a passed test whose consoles complain (vm::complaints, release and debug kernels alike), parser pinned by consoles_fail_on_their_kernels_complaints. The debug kernel (lockdep, KCSAN, DEBUG_ATOMIC_SLEEP: FXDAX_GUEST_KERNEL_BUILD=debug, existing) found fxfs_dax_window_page's dax_direct_access without dax_read_lock (fixed, 2c... this round); the DAX suites then pass on it (GCE r81). Left: a KASAN/KFENCE variant (KASAN excludes KCSAN), and TASK-99's race put back reported by it.
<!-- SECTION:NOTES:END -->
