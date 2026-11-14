---
id: TASK-395.15
title: 'fxdax-gc''s sweep is fastest at 4 passes at once: 16 and 64 are slower'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13 23:50'
updated_date: '2026-11-14 03:12'
labels:
  - collector
  - performance
dependencies: []
parent_task_id: TASK-395
priority: medium
ordinal: 789000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1 run t395sweep on main 22602dbd1 (docs/measurements/ubu1-gc-sweep-2026-11-13T2259Z-50e7eb9e), 1.0M objects in 10k cells of a file bucket: first sweeps took 629 s at 1 pass at once, 320 s at 4, 492 s at 16, 395 s at 64. The sweep's passes share one task (TASK-395.5's notes), so their CPU work (digests, decoding, compaction) runs on one thread; the file bucket's own costs may add. Find the bottleneck (profile, perf on ubu1 under fxjob --bench) and make the sweep scale with passes at once, e.g. passes spawned on a multi-threaded runtime.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A profile of the 16-at-once sweep names where its time goes
- [x] #2 Sweeps get faster up to at least 16 passes at once on ubu1's file bucket, measured with scripts/gc-sweep-host.sh
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Profile locally: release fxdax-gc over a file bucket of 100,855 objects in 1,000 cells (fxdax-gc-fill 10 100 95 950), first sweeps at 1/4/16/64 at once, macOS sample of the 16-at-once run.
2. Fix what it shows. Found: the file bucket's one bucket-wide flock, held by updates over their fsync (3-4 lease updates a pass) and by deletes; ~12 of 16 passes' threads waiting on it; the runtime thread 92% idle. Fix: a lock per directory (64 lock files by FNV-1a of the object's directory), deletes grouped by lock.
3. Also run passes as tasks of their own on a multi-thread runtime (Sweep::spawned, fxdax-gc), as the sweep's one task capped CPU work at one thread; make the pass futures provably Send (closures returning futures in streams replaced by fns).
4. Tests: local::objects_of_a_directory_share_their_lock, updates_wait_only_for_their_own_lock; sweep::spawned_sweeps_run_each_pass_as_a_task_of_its_own.
5. Re-time locally; give the parent the ubu1 command (scripts/gc-sweep-host.sh) and a perf recording to confirm the profile there.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), commit 286f4cb43 on branch t3/395-sweep-speed (one batch with TASK-395.16).
Profile (macOS, M-series, APFS; release fxdax-gc over fxdax-gc-fill 10 100 95 950 = 100,855 objects in 1,000 cells; /usr/bin/sample of the 16-at-once first sweep, 8 s): ~12.4 threads on average blocked in flock on the bucket's one lock (.fxdax-lock), ~5.7 in fcntl(F_FULLFSYNC), unlink/linkat/open ~3 each; the current-thread runtime's thread in kevent 92% of samples (idle). The lock is held by each lease update over its fsync (3-4 a pass) and by each batch of deletes over its unlinks, so passes over different cells queued on one another's fsyncs: the sweep's ceiling, more passes adding only contention (sys time 125 s of 233 s wall).
Changed: local::lock_of, a lock a directory (64 files .fxdax-lock.NN, FNV-1a of the directory path), deletes grouped by lock, each held until its deletes answer, renames under both in order (doc-14; OPEN_QUESTIONS.md: processes of older builds lock .fxdax-lock alone). Sweep::spawned: each pass a tokio task; fxdax-gc on a multi-thread runtime. Rust couldn't prove a pass's future Send through streams mapping closures to futures (higher-ranked lifetimes): delete_all's batches, compactions and trailer reads now build their futures by fns first (no boxing needed).
After, 16 at once: flock 922 samples (was 22,329); what's left is APFS work (F_FULLFSYNC, unlink, open; sys 205-303 s) and tokio's blocking pool (~500 threads) contending its own mutex.
Local first sweeps, seconds, 1/4/16/64 at once: before 330*/217/233/176; after 340*/226/177/122 (* 1 at once extrapolated from 120-133 cells' pass times, 0.33/0.34 s a pass). Peak RSS 17-33 MiB before, 25-67 MiB after (64 at once 67 MiB). LISTs 4,000 + 1,011 delimited (TASK-395.16).
Tests: local::objects_of_a_directory_share_their_lock (pins the hash: <C>gc/lease -> .fxdax-lock.21, 16 cells' leases on 16 locks, 4,096 directories cover all 64), local::updates_wait_only_for_their_own_lock (an update goes while another holding holds another directory's lock; one under the held lock waits until released), sweep::spawned_sweeps_run_each_pass_as_a_task_of_its_own (10 cells: 10 tasks spawned, 1 inline, same deletes); tests/gc.rs filters lock files.
Verified: see TASK-395.16's notes (same batch; scripts/test.sh parts all pass, Linux cross-check clean).
To measure on ubu1 (AC #2), from a clean tree: FXDAX_HOST_RUN=<run> FXDAX_HOST_COMMIT=286f4cb43 FXDAX_BENCH_NOTE='TASK-395.15/.16: lock per directory, passes spawned, 4 LISTs a pass' scripts/gc-sweep-host.sh
To confirm the profile there (AC #1), under the same fxjob --bench job, after the fill: a 16-at-once sweep of a copy run as 'perf trace -s -o trace.txt -- fxdax-gc file://COPY/ --grace 0 --at-once 16' (syscall time by thread: flock and fsync before, fsync/unlink after) and 'perf record -F 499 -g -o perf.data' of the same for on-CPU time of the runtime's threads; with the build before (c9b5ba549) for the A/B.

AC #1 by the delegate's macOS profile at 16 at once (passes blocked on the file bucket's one lock, held across each update's fsync and each delete batch; the runtime thread 92% idle). AC #2: ubu1 sweep t395sweep2 on 79c8249e1, first sweeps 602/559/190/112 s at 1/4/16/64 at once against 629/320/492/395 s before; second sweeps 281/178/92/53 s; peak 23-95 MiB. 4 at once slower than before, one run each: TASK-395.17. ubu1 run t395w6 on 79c8249e1 (workspace 2627/2627, sim 61/61, Firecracker 30/30 and 31/31, mutants 0 broken, guests pass); macOS scripts/test.sh passed; scripts/upstream-changes.py --check passes. On main as 79c8249e1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The sweep was bound by the file bucket's single lock, held across fsyncs, not CPU: the file bucket now locks per directory (64 stripes), and fxdax-gc runs each pass as its own task on a multi-thread runtime. On ubu1 over 1M objects in 10k cells, first sweeps at 16 and 64 at once went from 492 and 395 s to 190 and 112 s. Old and new builds must not write one file bucket at once (OPEN_QUESTIONS.md). Follow-up TASK-395.17 (4 at once slower, to re-measure). ubu1 run t395w6 on 79c8249e1 (workspace 2627/2627, sim 61/61, Firecracker 30/30 and 31/31, mutants 0 broken, guests pass); macOS scripts/test.sh passed; scripts/upstream-changes.py --check passes. On main as 79c8249e1.
<!-- SECTION:FINAL_SUMMARY:END -->
