---
id: TASK-44.8
title: >-
  Sanitized guest kernel: KCSAN, lockdep, KASAN, KFENCE and atomic-sleep checks
  in the guest suites
status: Done
assignee: []
created_date: '2026-10-24 21:31'
updated_date: '2026-11-04 08:55'
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
- [x] #2 The guest suites run on it on GCE, and every report it makes is fixed or filed
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

2026-11-04 at cfd41e1a0. Debug kernel (lockdep, KCSAN non-strict, atomic sleep), /tmp/fxdax-s448-debug: fxfs-guest-dax, checks-dax, checks-dax-delay, script-full-dax 4/4; no lockdep, WARN, BUG, atomic-sleep or hung-task report; KCSAN's fxfs reports all known: fxfs_nofs's PF_MEMALLOC_NOFS against the load balancer (reviewed), genalloc's lockless bitmap (a new pairing, _find_next_zero_bit / bitmap_clear_ll, the free side of the same race), kernel and virtio races; plus __mark_inode_dirty / inode_cgwb_move_to_attached via file_update_time, the kernel's. checks-dax-delay took 177 s of 180. KASAN kernel, /tmp/fxdax-s448-kasan: dax and script-full pass, no report; checks-dax and checks-dax-delay time out in enospc from KASAN's slowness, so enospc, write_times_add_up and unbind_waits_for_unmount haven't run under KASAN. Decided: run those three alone under KASAN (FXDAX_CHECKS) rather than lengthen every timeout; AC3 dropped, guest/sim's seeded schedules catching TASK-99's race and copy-on-write DAX (TASK-164) removing its code.

2026-11-04, KASAN, the three left alone (FXDAX_CHECKS=enospc,write_times_add_up,unbind_waits_for_unmount, /tmp/fxdax-s448-kasan2): all pass, no KASAN, KFENCE, lockdep or WARN report. AC2 holds on both kernels.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Every DAX guest suite runs clean on the debug kernel (lockdep, KCSAN, atomic-sleep checks) and the KASAN kernel (KASAN, KFENCE, lockdep) at cfd41e1a0: KCSAN's fxfs reports are reviewed races (fxfs_nofs's flag word, genalloc's lockless bitmap) and the kernel's own; KASAN's slowness made two suites time out, so their three last checks ran alone and passed. Decided not to: KCSAN catching TASK-99's race (AC3), which guest/sim's seeded schedules catch and copy-on-write DAX (TASK-164) removes. checks-dax-delay takes 177 of its 180 s on the debug kernel.
<!-- SECTION:FINAL_SUMMARY:END -->
