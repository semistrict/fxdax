---
id: TASK-149.18
title: 'Simulator invariants: every acknowledged object exists, every waiter is woken'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 16:53'
updated_date: '2026-11-06 04:51'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 535000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Taken to guest/sim, as the user asked (2026-11-05); extent_remote's simulation, which this first named, is TASK-149.32. guest/sim checked what its runs read back: a write lost shows only if a read reaches it, a waiter never woken only as a slow run, as dax.c's waits all have time limits, and an object a file names that the device doesn't hold only if read. Stated as invariants: every waiter is woken; every acknowledged write is durable in the model; every object a file names exists.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A timed wait that ends by its time, its condition true, fails the run (a wake lost); no task ends with a wait entry queued
- [x] #2 Every byte a write, store, truncate, punch or clone told a task is read back so, and in Fxfs's extents after an fsync that returned 0 and at the end of every run (the file model)
- [x] #3 At the end of every run each block a file names is of an extent the store holds, written that far, the extents' counts as the files say, and the window maps nothing but what failed UNMAPs kept
- [x] #4 A lost wake, a lost write, and a range's free that no waiter hears of, put back, each fail it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Done in guest/sim, as asked (2026-11-05), beside 149.15's sweep:
1. Every waiter is woken: a timed wait ending by its timer with its condition true is a lost wake (the sim's schedule_timeout wakes it once early, as a spurious wake, and finish_wait finds it still queued); no task ends with a wait entry queued.
2. Every acknowledged write durable: a file model in the sim's VFS (kernel/shadow.c), what read(2), write(2), stores, loads, truncates, punches and clones told tasks; a successful fsync, and the end of a run's sync, must find in Fxfs's extents every byte no task changed meanwhile.
3. Every acknowledged object exists: the model of Fxfs checks at each seal and at the end that each block a file names is of an extent the device holds and has written that far, its counts as the files' blocks say; the device's private pages as dax.c counts them.
4. Mutants of a lost wake and a lost write, killed.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Done in guest/sim with TASK-149.15. Waiters (kernel/sched.c): a timed wait whose time ends its sleep, its entry queued, gets 1 jiffy back as a spurious wake; finish_wait with the entry still queued after that means its condition was true with no wake: a lost wake. A wake reaching a task already running clears it. No task ends with an entry queued. File model (kernel/shadow.c): per ino, bytes and known bits per page, a busy count and clock per page and for the size; mount.c's calls and mm.c's stores and loads tell it; reads and loads are checked where stable; a 0 fsync and the end's sync check every known byte and the size against fxfs_sim_read. A writeback failure (mapping_set_error) forgets what changed since the file's last good fsync, and marks calls under way overlapped. Objects: the model's check at fxfs_sim_free that every block a file names is of an extent the store holds written that far and each extent's live count is what files name; the device after unmount maps nothing but failed UNMAPs' pages, its private count matching. Mutants: task-149.18-round-unwoken (interleave seed 5) and task-149.18-release-unwoken (read_pressure), both 'a lost wake'; the lost writes are TASK-149.15's 1 and 2. The file model also kills many old mutants first (task-164.4-swap-shared, -fsync-skips-writeback, -writeback-unlocked, -commit-failure-dropped, task-79.53-gap-left, unmapped-page-read-as-hole). Cost: interleave and failures about 1.4x CPU.

Verified at 98e260378 (dax.c at 814b0ccc2): guest/sim/mutants.sh --check 37/0 broken, scripts/guest-mutants-gce.py --check 43/0; zloop.sh -s 3000: 0 of 6,000 seeded runs failed, every run of one passed (7m52 at -j14); sweep.sh: 36 runs, 13,680 calls each failed in turn, 0 failed (234 s at -j14); mutants.sh: every sim mutant killed but the six whose killer is none; crates/fxfs-sim-diff tests and its mutants.sh as before; macOS workspace (scripts/test.sh workspace) 2118/2118; GCE r700 (us-central1-b, jailed, seccomp real, guest kernel built on the host): fxfs-guest-dax, -checks-dax, -crash-dax, -pgbench-dax-forked pass, no unexplained refusals, host deleted; scripts/upstream-changes.py --check passes. Linux workspace not run (no Rust changed).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/sim's invariants: lost wakes fail a run (a timed wait's early probe), no task ends queued; the file model checks every read and load, and that every acknowledged write is in Fxfs after a good fsync and at every run's end; the model's blocks all name extents the store holds, the window empty after unmount. Mutants of a lost wake (two) killed; the lost writes it found are TASK-149.15's. extent_remote's part is TASK-149.32. Linux workspace DoD not run: no Rust changed.
<!-- SECTION:FINAL_SUMMARY:END -->
