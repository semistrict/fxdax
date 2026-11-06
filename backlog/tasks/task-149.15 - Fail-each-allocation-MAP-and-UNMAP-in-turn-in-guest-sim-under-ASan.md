---
id: TASK-149.15
title: 'Fail each allocation, MAP and UNMAP in turn in guest/sim, under ASan'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 16:53'
updated_date: '2026-11-06 06:56'
labels: []
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 532000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The fork sweep failed each step in turn and found 7 bugs. TASK-44.10 injects at a probability on real kernels, which samples; this enumerates, in a process, with seeds, on the paths two audit findings and TASK-99 lived on. Would find the double free, the unmount panic and stale ranges after a failed MAP.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 guest/sim fails the Nth allocation, MAP, UNMAP and device call for every N a scenario reaches, one-shot
- [x] #2 Each run is checked by ASan, dax.c WARNs and the file model
- [x] #3 A past bug of this class, put back, fails it
- [x] #4 A hunt has run, its findings fixed or filed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One place decides what fails (kernel/sweep.c): every site the code under test may see fail on a real kernel asks it, allocations by fs/fxfs, fs/dax.c and the xarrays they use, the device's MAP, UNMAP and PRIVATE, and Fxfs's reserve, growth, commit, clone, seal and lookup; SIM_SWEEP=N fails the Nth, once, and every run says how many it reached.
2. A test's expectations are void once the sweep's call failed (sim_unexpected): the run goes on to the invariants, which stay hard: ASan, UBSan, WARNs, the sim's own checks, and the end of run (sim_end): the mount synced, every file reading as Fxfs holds it, what it was told durable (TASK-149.18's file model), unmounted, leaks (LSan).
3. sweep.sh: each run of one (every test of one run, every scenario), clean to count its calls, then each N; mutants.sh's killer 'sweep RUN'; a CI step within guest-sim's time.
4. A past bug of the class put back, killed by the sweep.
5. Run it over every scenario; fix what it finds in dax.c, data.c, pages.c with a scenario or mutant each, or in the sim's model.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design: kernel/sweep.c is the one place a call of the code under test asks whether it fails (allocations by fs/fxfs, fs/dax.c, lib/xarray.c, lib/genalloc.c; MAP, UNMAP, PRIVATE; the model's reserve, growth, commit, clone part way, seal, lookup). SIM_SWEEP=N fails the Nth once; sim_end says how many a run reached; sweep.sh runs every run of one clean, then each N (36 runs, 13,680 calls). After the failure the test's expectations are void (sim_expect/sim_await) and the run goes on to the invariants: ASan, UBSan, LeakSanitizer (now on for every guest/sim run), WARNs, the sim's checks, and sim_end (sync, files read whole against Fxfs, the file model's durability, unmount, the window mapping nothing but failed UNMAPs' pages, the model's blocks). A run stuck at a pause point or hold its test can't release 300 s after the failure ends void. mutants.sh and guest-mutants-gce.py take 'sweep RUN' killers; CI job guest-sim-sweep (234 s at -j14 locally; a job of its own, as guest-sim's 3,000 seeds take most of its 45 min).

Findings in dax.c, each fixed with a scenario and a mutant, each mutant red:
1. fxfs_dax_write_back sized its batch with the semaphore shared and swapped no more than that under it held for writing: a store between dirtied another range, the swap took it and passed the earlier one by, the fsync returning 0 without bytes written before it began (failures seed 62, found by TASK-149.18's file model). Blind spot: no oracle checked durability at every fsync for every file, and no scenario had two ranges of a file dirtied across the count. Fixed by counting again under the write lock; pause point 10 COUNTED; cow_writeback_counts_again; task-149.15-writeback-batch-short. Structure: a count and its fill in two critical sections; fxfs_dax_busy_files counts and fills under one lock, the only other such pair.
2. fxfs_dax_busy_files' allocation failing made fxfs_dax_settle write nothing and return 0 (sweep of short_copy, the end's sync). -ENOMEM now; relieve passes its round by. task-149.15-sync-without-memory, killed by 'sweep scenarios short_copy'.
3. The budget worker's round called fxfs_dax_released, resetting every wait for room's idle count: a write that could never have room (aligned room taken by failed UNMAPs, read_pressure's 8 MiB window) waited for ever (livelock). The round no longer counts as a release; its writeback's ranges release as they're let go. unmaps_failed_take_the_window; task-149.15-round-releases. Left: a window whose aligned room failed UNMAPs took serves no chunk's range though most is free: TASK-149.33.
The sim's own: a failed mount went on; struct files and the model's extent refs leaked; ended tasks' stacks were kept until a run ended (GBs, OOM-killed); a sync's s_wb_err told twice; scenarios' unbounded waits for holds (sim_await now); cow_store_to_pinned_slice unpinned NULL. Five writeback mutants rewritten against fix 1 and 2, all still killed.

Verified at 98e260378 (dax.c at 814b0ccc2): guest/sim/mutants.sh --check 37/0 broken, scripts/guest-mutants-gce.py --check 43/0; zloop.sh -s 3000: 0 of 6,000 seeded runs failed, every run of one passed (7m52 at -j14); sweep.sh: 36 runs, 13,680 calls each failed in turn, 0 failed (234 s at -j14); mutants.sh: every sim mutant killed but the six whose killer is none; crates/fxfs-sim-diff tests and its mutants.sh as before; macOS workspace (scripts/test.sh workspace) 2118/2118; GCE r700 (us-central1-b, jailed, seccomp real, guest kernel built on the host): fxfs-guest-dax, -checks-dax, -crash-dax, -pgbench-dax-forked pass, no unexplained refusals, host deleted; scripts/upstream-changes.py --check passes. Linux workspace not run (no Rust changed).

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**guest/sim's sweep and file model: what a run may do once something failed (TASK-149.15, TASK-149.18, 2026-11-06)**

- **TASK-149.18 is guest/sim's, as asked**: its invariants (every waiter woken, every acknowledged write durable, every object a file names held by the device) are guest/sim's; extent_remote's, which the task first named, are TASK-149.32 with its old criteria. *Undoing it:* move the criteria back.
- **Once the sweep's call has failed, a test's expectations are void and the run goes on** (`sim_expect`), to the invariants and the end of the run (`sim_end`), rather than ending at the first one: what comes after a failure is where its bugs show. A run still going 300 s of virtual time after, every task asleep, at a pause point or a hold its test can't let go of, ends there, void. *Instead:* end every run at its first void expectation, which would skip the end's sync and unmount. *Undoing it:* `sim_unexpected` panicking.
- **A writeback that fails loses what changed since the file was last durable, at once, as far as the file model knows**: copying on write, its pages are clean and private, and a rebind, a drop or a truncate shows what Fxfs holds whenever it comes, as the page cache's clean pages after a failed writeback may be dropped. The error is told by the next fsync and the next syncfs (`s_wb_err`, which the sim now keeps). A file whose writeback failed isn't compared, read(2) against Fxfs, at the end. *Instead:* have dax.c keep a failed writeback's pages dirty, so nothing is lost before an fsync tells, which no kernel filesystem does. *Undoing it:* `lost()` at the fsync's error alone.
- **Every guest/sim run checks for leaks as it exits** (LeakSanitizer, `sim_prepare`), the sim freeing its own; zloop's seeds too. *Undoing it:* `detect_leaks=0` in lib.sh.
- **A sweep's allocations are wider than `tests/failures`' odds**: fs/fxfs's, fs/dax.c's, and the XArrays' and pool's they use; the odds stay fs/fxfs's alone, so seeds run as they did. And the model of Fxfs fails a clone part way, a seal and a lookup, which only the sweep asks for.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
guest/sim fails each call of the code under test that may fail, in turn (kernel/sweep.c, sweep.sh, SIM_SWEEP=N): allocations, MAP/UNMAP/PRIVATE and the model of Fxfs's transactions, 13,680 runs over every run of one, checked by ASan, UBSan, LeakSanitizer, WARNs, the file model and sim_end. Found and fixed three dax.c bugs (an fsync missing a range dirtied before it, a sync without memory returning 0, a livelock waiting for window room), each with a scenario and a mutant; CI job guest-sim-sweep. Linux workspace DoD not run: no Rust changed.
<!-- SECTION:FINAL_SUMMARY:END -->
