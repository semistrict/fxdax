---
id: TASK-378
title: >-
  ubu1: ordinary work is OOM-killed at 0.6-1.6 GB while a measurement holds its
  16 GB: user-1000.slice's MemoryMax 21G leaves all other work about 5 GB
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 10:27'
updated_date: '2026-11-12 13:04'
labels:
  - bug
  - ubu1
dependencies: []
priority: high
type: bug
ordinal: 760000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-12, step 146 (system-setup.d/146-fxdax_cgroups.sh, TASK-371's session's design, OPEN_QUESTIONS 'ubu1: work best effort, measurements isolated by the kernel'): user-1000.slice MemoryMax=21G, fxdax-bench.slice MemoryMin=16G. With TASK-330.4's run in fxdax-bench.slice at 16.3 GB, three fxjob work scopes were OOM-killed within the hour: t371-kernel (vmlinux link, peak 598 MB, with t364-fc killed in the same second), t371-fxdax-tests (cargo build, peak 939 MB), t371-ci-fix (cargo nextest -p fxdax, peak 1.6 GB, --mem 12G). free -g showed 15 GB available host-wide meanwhile: the cap is the user slice's, not the host's. So cargo builds of the workspace can't run on ubu1 while a measurement runs, which is most of the day. Options: raise user-1000's MemoryMax to what the host can give (27 GB total, ZFS ARC capped at 4G), or make it MemoryHigh, keeping the bench's MemoryMin; check what the 21G cap was protecting (the host's own services) before.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A cargo build of the fxdax workspace's tests under fxjob runs to completion while a bench job holds 16 GB
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Find what user-1000's MemoryMax 21G protects and the largest safe value: host 27.7 GiB less kernel outside cgroups, ARC, system.slice/gdm, margin.
2. Let fxdax-work.slice's jobs swap (slice MemorySwapMax 6G); move no-swap from fxdax.slice to fxdax-bench.slice and keep each bench job's MemorySwapMax=0; MemoryHigh 16G on work and MemoryMin on bench unchanged.
3. Raise user-1000 MemoryMax 21G -> 24G, counting the ARC at its floor (zfs_arc_min) rather than its cap: swap alone doesn't save a build whose working set exceeds its RAM.
4. Step 146: apply slice changes by daemon-reload (verified live on a probe slice), reexec only for the delegate drop-in; fxjob ps shows swap; header comment updated.
5. Deploy beside r3305 (its scope already swap 0): write new file, mv, run system-setup.sh.
6. Prove: a workspace cargo build --tests as ordinary fxjob work (default 8G) completes while a measurement holds ~16 GB in a sandboxes part; report its peak, swap, and the measurement's memory; bench jobs still MemorySwapMax=0.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-12 host facts (ubu1): MemTotal 28,375 MiB; 8 GiB swap; ARC cap 4 GiB, floor (zfs_arc_min default) 887 MiB; kernel memory outside every cgroup ~1.5 GiB (MemTotal-MemFree less root memory.stat's anon+file+kernel, less ARC); system.slice ~0.17 GiB, gdm 0.1, init 0.01. With the ARC at its cap, the room for user-1000 is ~21G: the old cap was already that. With the ARC at its floor (the rest is cache the host's reclaim shrinks), 28,375-1,536-887-512-512 = 24.9 GiB: user-1000 MemoryMax raised to 24G. memory.min holds in global reclaim too, so a measurement isn't paged by it; the system's idle pages and the ARC pay.
Found: fxdax-work.slice held 3.2 GB of shmem with no job running: test TempDirs leaked in /tmp (tmpfs) by killed tests, charged to 110 dying work memcgs that ran with swap.max 0, so nothing reclaims them but deleting the files (TASK-379 filed).
Deployed 05:48 EST: fxdax.slice no longer MemorySwapMax=0; fxdax-work.slice MemorySwapMax=6G; fxdax-bench.slice MemorySwapMax=0; fxjob gives MemorySwapMax=0 only to --bench jobs. Applied by systemctl --user daemon-reload (a probe slice showed reload writes changed slice properties to running cgroups); r3305's scope kept swap.max 0, partition root throughout. fxjob ps has a SWAP column.
Swap alone isn't enough: a workspace cargo build --tests (CARGO_BUILD_JOBS=4, nice 19) held to --mem 2G swapped 0.7 GB (pswpout 195k, pswpin 104k), thrashed and was OOM-killed at its limit at 05:56 (anon 1.77 GB, swapcached 0.39 GB). So 05:58 EST: user-1000 MemoryMax 21G -> 24G (step 146 rerun, only that drop-in replaced).

First squeeze run (06:04:49-06:15:27 EST), user-1000 at 24G, work swap allowed: the waiter started a fresh workspace cargo build --tests (CARGO_BUILD_JOBS=4, nice 19, UCD_ZIP set, fxjob default 8G) once r3305's scope held 14,903 MiB (its last part, main's bucket soak, not a sandboxes part). It finished in 10m34s, exit 0, no OOM kill: job peak 8,192 MiB (its own MemoryMax; memory.events max 7,527), swap peak 1,484 MiB (pswpout 846k, pswpin 672k, pgmajfault 6.8M), host swap 565-2,475 MiB. r3305 held 454-16,647 MiB over the run and ended at 06:13:43, so it overlapped 8m53s of the build. Without a time series, it doesn't show that the user slice's cap (rather than the job's own 8G) squeezed it: rerun with per-2 s series (job, measurement, user slice and its max events, swap) on the next qualification round's sandboxes part. Before it, the coordinator cleaned /tmp (TASK-379): ~1.7-1.9 GB of dead tmpfs charge freed, so this run had that much more room than the case filed. fxjob ps glitch after a measurement ends filed as TASK-381.

Squeeze attempt 1 (own holder, 06:25-07:03 EST): fxjob --bench --mem 16G stress-ng --vm 1 --vm-bytes 15872M --vm-keep, beside a fresh workspace build (default 8G). The holder was OOM-killed at its own 16G after 6 min (stress-ng's anon grew to 16.35 GiB with --vm-method all). During those 344 s: bench slice 17,222-17,602 MiB (incl. ~1.5 GB charged to ended bench jobs), user-1000 up to 24,371 of 24,576 MiB, but the build was only up to 1,492 MiB: not the proof. The build finished alone (37m21s, exit 0, peak 8,192 MiB at its own limit, swap peak 715 MiB). Attempt 2: holder at --mem 17G, --vm-method write64.

Proof, squeeze attempt 2 (07:10:43-08:03:32 EST): fxjob --bench --mem 17G stress-ng --vm 1 --vm-bytes 15872M --vm-method write64 --vm-keep (15.5 GiB anonymous, resident; MemorySwapMax 0) and beside it, once it held 15,907 MiB, a fresh workspace cargo build --workspace --tests as ordinary fxjob work at the default 8G (CARGO_BUILD_JOBS=4, nice 19, own CARGO_TARGET_DIR), sampled every 2 s. While the measurement held >= 14 GiB (2,996 of the build's 3,168 s; bench slice 15,351-17,213 MiB, incl. ~1.5 GB charged to ended bench jobs): user-1000 up to 24,575 of its 24,576 MiB, its memory.max hit 8,004 times, at >= 24,000 MiB in 551 of 1,582 samples; the build up to 4,732 MiB, its own max hit 0 times (the slice's cap squeezed it, not its own limit), swap up to 2,123 MiB (pswpout 1.17M, pswpin 1.03M); host swap up to 3.7 GB, MemAvailable down to 2.4 GB. It finished, exit 0, in 52m46s (10m34s with no squeeze): slower, not killed. No OOM kill anywhere on the host meanwhile (kernel log 07:10-08:04). The holder ran its full 50 min (its stress-ng timeout ended it 3 min before the build, not my kill) and kept 15.5 GiB, unswapped. Every bench job still gets MemorySwapMax=0: ab-main-1 (another agent's, started after the change) showed MemorySwapMax=0, swap.max 0; fxdax-bench.slice swap.max 0. This ran after the coordinator's /tmp cleanup (TASK-379), which freed ~1.7-1.9 GB of dead tmpfs charge from work: the case as filed had that much less room.
Step 146's header comment records the limits, why, and these numbers (the step rerun at 08:04 replaced nothing). DoD #1 not run: no code in this repo changed (ubu1's system-setup step only).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
ubu1 step 146: work may swap (fxdax-work.slice MemorySwapMax 6G; no-swap moved from fxdax.slice to fxdax-bench.slice, kept on every --bench job), and user-1000's MemoryMax raised 21G -> 24G, counting the ARC at its floor (0.9 GiB) rather than its 4 GiB cap: 27.7 GiB less ~1.5 kernel, 0.9 ARC, 0.5 system, 0.5 margin. Swap alone wasn't enough (a build held to 2G thrashed and was OOM-killed). Slice changes now apply by daemon-reload, reexec only for the delegate drop-in; fxjob ps shows swap. Verified: beside a 15.5 GiB measurement for 50 min, a workspace cargo build --tests at the default 8G kept user-1000 at its cap (8,004 max events), peaked 4.7 GB, swapped 2.1 GB, and finished in 53 min, no OOM kill on the host; bench jobs still MemorySwapMax=0. Follow-ups: TASK-379 (killed jobs' tmp files), TASK-381 (fxjob ps errors).
<!-- SECTION:FINAL_SUMMARY:END -->
