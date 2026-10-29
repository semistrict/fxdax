---
id: TASK-44.8
title: >-
  Sanitized guest kernel: KCSAN, lockdep, KASAN, KFENCE and atomic-sleep checks
  in the guest suites
status: To Do
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-10-29 22:04'
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
- [x] #1 build-guest-kernel.sh builds a sanitized guest kernel (KCSAN, PROVE_LOCKING, KASAN, KFENCE, DEBUG_ATOMIC_SLEEP) on a switch
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

FXDAX_GUEST_KERNEL_BUILD=kasan (KASAN, KFENCE, lockdep, no KCSAN): the kernel builds, but triple-faults at boot under Firecracker (vCPU exit Shutdown within 1 ms, an empty console; GCE r84): a KASAN kernel booted directly by Firecracker needs looking into (its early shadow, the boot path).

2026-10-29: the KASAN kernel boots under Firecracker built without PVH, entered at its 64-bit entry (88032eb67; GCE r253 showed it faulting before its first line at PVH's, r256 booting without). GCE r257 on it: fxfs-guest-dax and fxfs-guest-crash-dax passed; checks-dax and -small outran their 180 s timeout (KASAN's slowness), checks-dax's console with map_to's gate assertion firing (TASK-44.21, narrowed since: r258 none); no KASAN or KFENCE report.

AC3, GCE r261 (mutant region_open_unlocked, TASK-99's race put back, on the debug kernel, FXDAX_MUTANT_KERNEL=debug): killed in round 1 of checks-dax, but not by KCSAN: the VM stopped (the guest's output empty) and test-e2e.sh's check found refused stores no seal explains (scripts/dax-refusals.py, TASK-44.12), TASK-99's symptom. KCSAN, non-strict (CONFIG_KCSAN_STRICT off), reported no race in region_open or map_to; its reports were the reviewed PF_MEMALLOC_NOFS change (fxfs_dax_write_size_locked / kthread_is_per_cpu) and two of the kernel's. AC3 open: KCSAN_STRICT, or a race window held open (a pause point in region_open), may let KCSAN see it.

2026-10-29, GCE r255 debug kernel (KCSAN): fxfs-guest-checks-dax reported fxfs_dax_cow's unlocked find_next_bit racing another read iomap's test_and_clear_bit of range->relocated: benign, marked data_race (TASK-44.22's commit). Also seen, not ours: genalloc's lockless bitmap (_find_next_bit / gen_pool_alloc_algo_owner), virtio_ring, workqueue.
<!-- SECTION:NOTES:END -->
