---
id: TASK-44.3
title: Shuttle over extentd's DAX window and userfaultfd remapping
status: Done
assignee:
  - '@claude'
created_date: '2026-10-18 04:06'
updated_date: '2026-10-18 06:20'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: high
ordinal: 132000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extentd's DAX window (crates/fxdax/src/dax.rs, dax/uffd.rs, the window in extents.rs) resolves faults concurrently, as futures, on fault threads, while the extent store maps and remaps pages; a remap takes several system calls, between which a vCPU's access fails with EFAULT and is retried (Firecracker's vCPU retry). uffd.rs has one test. Its system calls are behind a trait, SecondStage, so a fake second stage can run the real code under Shuttle (doc-8 testing 2 explains Shuttle and why not loom).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A fake SecondStage and the window's real code run under Shuttle (fuchsia-sync and the fault futures on Shuttle's primitives with a shuttle feature), with faults (reads, writes, a write after a read) arriving between every step of maps, remaps to read-only, unmaps and moves
- [x] #2 Invariants checked: a fault resolves to the page the window maps now, or fails with EFAULT only inside a remap and succeeds on retry; no store goes untracked (a page written is dirty); no page of zeros where data was
- [x] #3 Mutation checks: each ordering rule of the remap (register before map, write-protect before read-only, ...) turned off makes a Shuttle test fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Spike first (one day, user-approved 2026-10-18): 1. zircon gets a 'shuttle' feature: parking_lot becomes shuttle-parking_lot 0.12.5 (Apache-2.0; parking_lot's own without the feature), and crate::sync gives a Condvar over lock_api guards and threads (std's, or Shuttle's). 2. Kernel objects use crate::sync's Condvar and threads. 3. Run fxdax's faults_wait_concurrently (window, pager, port) under one Shuttle schedule, then many. Exit: if the kernel's background threads (timer, evictor, scanner, page queues) or its clock can't be made to run under Shuttle within the day, stop and model the remap ordering in TLA+ instead. 4. If it runs: FakeStage (SecondStage recording each page's mapping), vCPU/pager/store threads racing faults against map, make_read_only, unmap and replace_kept; dax Rules switches, a mutation test each.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Spike (2026-10-18) succeeded:
- zircon 'shuttle' feature: parking_lot is shuttle-parking_lot 0.12.5 (Apache-2.0; parking_lot itself without the feature), so its 25 files of locks are unchanged. crate::sync adds a Condvar over lock_api guards on Shuttle's primitives (the wrapper has none; generation counter so no notification is lost) and spawn(), through which the kernel's time-driven threads start (timer, wakeups, scanner, page queues, evictor, memory watchdog): under Shuttle none starts, as time doesn't pass (Shuttle's timed waits never time out). Without that they ran as OS threads and died silently on first use of a Shuttle lock (found with --no-capture).
- Kernel global state (OnceLock PMM, handle table, ...) reused across Shuttle runs works.
- fxdax dax/shuttle_tests.rs: kernel_waits_are_shuttles (a port wait with no waker is a reported deadlock: plumbing proof); faults_wait_for_the_pager; stores_race_seals_and_frees: FakeStage (SecondStage recording each page's phys/protection; accesses atomic under its lock, through a mapping of the page's arena), 2 vCPUs reading/writing and retrying faults until both pages read freed, store sealing (make_read_only) then freeing (replace_kept). Invariants: reads never go back a version; no writable page after the seal returns; sealed copy unmapped after the free returns; stores fail only once sealing began. Each run's events counted, and each test requires every event reached (reads of each version, faults, refused stores).
- Mutation check (manual): replace_kept unmapping instead of mapping zeros -> both Random and PCT fail with a read fault ENOENT. 2000 schedules each in ~3 s.
Remaining for the ACs: Rules switches in the window/kernel for each ordering rule with a mutation test each; moves (remaps between extents) and populate; the accessed-flag emulation (harvest) path; userfaultfd's ordering (register before map, write-protect before read-only) lives in uffd.rs, behind real syscalls, which this doesn't reach.

Finished after the spike:
- uffd.rs: the stage's system calls behind HostMemory (Syscalls the real one, with the userfaultfd; UffdSecondStage<H = Syscalls>, so callers are unchanged); its mapping and fault resolution generic over it. Its ordering rules switchable (uffd::Rules): map_with_window_protection, write_protect_before_writable, register_before_accessible, refused_store_read_only. dax::Rules: free_maps_zeros.
- shuttle_testing.rs: runs/config/fails_with shared by every module's Shuttle tests (open_stores' moved onto it).
- dax/shuttle_tests.rs: the race (two vCPUs storing and reading while the store seals, then frees) generic over a Guest; FakeStage's guest (HVF-like). dax/uffd/shuttle_tests.rs (Linux): the real UffdSecondStage over ModelMemory (per page: file or anonymous, prot, userfaultfd registration, write protection), told by a wrapper stage what the window asked of each page; vCPU accesses as KVM's: EFAULT where the protection refuses (retried), userfaultfd missing/WP faults resolved by the stage's real resolve(), a page of zeros where anonymous memory isn't registered (asserted never), a store the window wasn't told to allow (asserted never). Every rule turned off fails with its reason (untracked store x2, a page of zeros, a store never completing; ENOENT for the free). Reach checks include EFAULT retries. SHUTTLE_SOAK=20 (2,000 schedules each) clean in Lima.
- Found: the accessed-flag emulation never touches a window: only user aspaces are harvested (VmAspace::harvest_all_user_accessed_bits filters is_user), and windows are guest-physical.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The DAX window's real code, the kernel's, and the userfaultfd second stage's run under Shuttle. zircon's shuttle feature puts the kernel on shuttle-parking_lot's locks, with crate::sync's Condvar over Shuttle's primitives and its time-driven threads not started (time doesn't pass under Shuttle). The userfaultfd stage's system calls are behind HostMemory, so its real mapping and fault resolution run over ModelMemory, against which vCPUs' accesses are made as KVM makes them. Two vCPUs store and read while the store seals and frees, against an HVF-like fake stage and the userfaultfd stage: reads never go back a version, nothing writable after a seal, no sealed copy mapped after a free, no page of zeros, no untracked store, every access completes (EFAULT retried), each test's schedules required to reach every case. Each ordering rule turned off fails with what it prevents (uffd: window protection until tracked, write-protect before writable, register before accessible, refused store read-only; dax: free maps zeros). Verified: macOS scripts/test.sh (2,011 tests, Shuttle suites), Lima zircon+fxdax (275) and 15 Shuttle tests, 20 repeats clean; upstream-changes --check. Not covered: Dax::unmap racing accesses (a guest doesn't touch ranges it unmapped); the accessed-flag emulation never reaches windows (guest-physical aspaces aren't harvested); Linux --workspace ran for zircon and fxdax only, the crates changed.
<!-- SECTION:FINAL_SUMMARY:END -->
