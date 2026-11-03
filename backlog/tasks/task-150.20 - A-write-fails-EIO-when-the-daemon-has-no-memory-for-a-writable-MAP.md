---
id: TASK-150.20
title: A write fails EIO when the daemon has no memory for a writable MAP
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 21:51'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
parent_task_id: TASK-150
priority: high
type: bug
ordinal: 543000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
xfstests generic/129 on DAX: r452 in the suite, and r454 and r455 alone, on the merged tree and before the audit's fixes alike, first round, so older than today's work. ExtentStore's writable map commits an open extent's pages (crates/fxdax/src/extents.rs, 'bringing in'), which gives ENOMEM when memory is short; the guest's retry fails too, and the write says EIO, where it could wait or make room: an open extent's pages can't be evicted until sealed, so a seal may free them. Every earlier DAX xfstests run shows 2 such failures in the guest's log, survived; generic/129 alone reaches it at 10 s. Not in xfstests' known failures, on purpose. Unknown: which memory limit these runs meet, and why alone is worse than in the suite.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A test reproduces it without xfstests
- [x] #2 A write under memory pressure waits or makes room, and doesn't say EIO
- [x] #3 The notes name the blind spot and what a search around it found
- [x] #4 generic/129 passes on GCE, alone and in the suite
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Measure what holds memory at ENOMEM (r460, diagnostics in a frozen copy). 2. Reproduce with no VM: guest/sim's device gets host memory for open extents. 3. Daemon: zeros made with the window; no table lock across an allocation's wait; open pages pinned within a budget; unpin ends exhaustion. 4. Guest: a refused MAP seals and tries again, every way in. 5. Prove: unit tests, sim scenario and seeds, a mutant per way in, a real-kernel case, generic/129 alone and in the suite, the bug back on a real kernel.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
CAUSE (r460, diagnostics in a frozen copy, not on the branch): the limit is the daemon's physical memory, 1 GiB by default, which FXDAX_EXTENTD_MEMORY left unset in these runs. No commit between t=0.79 s and t=10.4 s; at the first ENOMEM open_live=262138 of 262144 pages, cached present=0, quarantined=0; relief freed 36 KiB once, then 0 each pass, and the arena went exhausted. The commit at 10.45 s sealed 512 extents, 262106 pages, and after it present=260570: sealed, they are evictable. So all of memory was open pages, which only a seal lets go of, and only the guest can seal. Fxfs seals on a sync or a journal compaction; nothing tells it the host is short.
WHY generic/129 ALONE REACHES IT: looptest -t -b 102400 truncates and writes 1 GB, each write's blocks new, never syncing, in about 8 s: faster than the journal fills. Most tests sync, or write less.
WHY ALONE IS WORSE: the task's premise that ENOMEM alone explains it was incomplete. Both solo logs (r454, r455) panic straight after the ENOMEMs: 'a page of zeros for DAX windows: NoMemory', cache.rs:983. Zeros were made at their first use, which on a solo run came with memory gone, holding the store's dax lock: it poisoned, and every later request of the guest's panicked, the 347 PoisonErrors in r454's log, 'Failed to free extent', unmount failing. In the suite an earlier test had made them: r452 has the ENOMEMs and no panic.
FIX: (1) Dax owns its zeros, made as the window is, ENOMEM there rather than a panic later. (2) OpenPages::commit waits for room without the table's lock: relief unmaps from windows, whose lock a fault holds waiting for the table's, a deadlock under exactly this pressure. (3) physmem pins: what relief can't evict, within 7/8 of what an arena, and the arenas together, may hold. A MAP past it says ENOMEM with memory left; writes and faults, which can't go back, go past it into the reserve. (4) unpin, under its pages being in the cached table, ends an arena's exhaustion, with a count so a pass racing it doesn't exhaust it again. (5) dax.c: fxfs_dax_make_memory: a MAP refused for memory seals (fxfs_fsync) and tries again, at each way in: write, fault, zeroing, read. Outside the gate and outside iomap_begin, as dead-ends/map-waits-for-a-seal.md says why. Sealing in vain says ENOMEM, never EIO. No retry was lengthened: each try follows a seal, which changes what the host holds.
BLIND SPOT: the path never ran with anything checking the result. No test wrote more than the daemon's memory between seals; capped_arenas_refuse_what_they_cannot_evict blessed the daemon's ENOMEM and stopped there. guest/sim's device had no memory at all, and its MAP failures were one-shot EIOs, so a failure lasting until a seal was never played. The suite hid the zeros panic behind another test's first use. And failing past memory wasn't tested down to the last page: my own first test passed with the bug back, as a failed write gives back the pages it found.
TESTS, each red with its bug back: guest/sim open_memory_out, and the host's memory limited on a seed in four of interleave and failures, so that class of generator now plays it. With the seal gone: scenario at 'write 60', 'mapping pages 0 to 25 ... failed: -12' as the real guest logged it; 200 of 200 limited seeds in each of interleave and failures. A mutant per way in, each found alone: write by the scenario, fault seed 8, zeroing seed 8, read seed 592, the rarest. Unit: memory_gone_leaves_the_store_going (lazy zeros, stale exhaustion), writable_maps_past_the_open_budget_are_refused_until_a_seal (no budget, pin never let go), waiting_for_room_leaves_the_table_unlocked (table lock), open_pages_are_pinned_until_they_go, pins_stop_at_their_budget, pins_let_go_of_end_an_arenas_exhaustion, pins_let_go_of_during_a_pass_leave_no_arena_exhausted. Real kernel: fxfs-guest-host-memory, a VM on a 128 MiB daemon writing twice that three ways, asserting dax_memory_seals rose for each. r464, the seal removed: generic/129 'WRITE: Input/output error', the case 'write 1120: I/O error'.
RESULTS: r461 (before the exhaustion fix) and r463 (head) generic/129 alone: 6 rounds each, 0 daemon panics; r463's daemon refused 30 MAPs, none reaching a writer. r462 full xfstests-fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-dax, fxfs-guest-host-memory: passed; xfstests' failures are r452's 17 less generic/129 exactly. zloop 800 seeds, macOS workspace 1965 of 1965, upstream-changes --check passes. Linux workspace in Docker 2114 of 2116: tenant_sockets_work_bind_mounted_into_jails (Docker only) and guests_forks_failing_at_each_step_harm_nothing, below.
LOOKED AROUND: ENOMEM turning into worse elsewhere: a read of a sealed extent falls back to the store, fine. A MAP in fxfs_dax_grow, restage or evacuation is tolerated. WRITE into an open extent past the reserve still fails, and the guest's errno() makes it EIO, fatal for a journal write. A fault that fails ENOMEM aborts the VM process (dax/uffd.rs, 'unresolved DAX window fault'): the reserve is what keeps both unreached, not a fix of either. Both need memory truly gone past an eighth of it, which a first refused MAP should stop. Two flakes met, both on the base too, neither this change's: windows_share_bases_pages (read_part's Arc outlives its waiter: 136 of 500 on the base, 41 of 500 on head, under load), and guests_forks_failing_at_each_step_harm_nothing (the uploader's own gather takes remote::gather's one failure: 10 of 10 on the base, 8 of 10 on head, under load). Mutants stood in for guest-coverage.py, which wasn't run.
STRUCTURE: small, done: what can't fail no longer shares a pool with what can (pins); a window's zeros can't be missing. Architectural, raised, not done: (a) only a guest can seal, and the daemon can neither evict an open page nor ask one to. On a shared daemon one VM's open pages, idle, can hold the budget, and another's seals free nothing: it gets ENOMEM, truthfully, but it's another's memory. Options: a device event asking a guest to seal, a per-tenant share of the budget, or open pages evicted to the store and faulted back. (b) A panic under a store's lock kills every later request of its guest: TASK-104, TASK-105, this one. All three were a lazily made resource and an expect; the class is std::sync::Mutex poisoning turning one panic into a dead store. (c) doc-4's MAP doesn't say ENOMEM means seal and try again: not edited, as this work touches only this task.
UNVERIFIED: several VMs at once; WRITE past the reserve on a real kernel; what a seal of 896 MiB costs a writer, not measured; read and zeroing refused on a real kernel, simulated only (the case's mapping way runs the fault path for real).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A write under the daemon's memory pressure no longer fails EIO: a writable MAP refused for room makes the guest seal and try again, with a test reproducing it without xfstests, and generic/129 passing on GCE alone and in the suite (r477). The blind spot (a lazily made resource and an expect under a store's lock, poisoning the store) and the search around it are in the notes. Unverified: several VMs at once, WRITE past the reserve on a real kernel, the cost of an 896 MiB seal to a writer; one VM's idle open pages holding a shared daemon's budget is TASK-153.15's question.
<!-- SECTION:FINAL_SUMMARY:END -->
