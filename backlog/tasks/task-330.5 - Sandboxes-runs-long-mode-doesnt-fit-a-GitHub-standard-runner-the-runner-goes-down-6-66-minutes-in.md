---
id: TASK-330.5
title: >-
  Sandboxes run's long mode runs on ubu1 by hand before each pin, not nightly in
  CI; it must finish there (it ran 2+ h unfinished)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 07:25'
updated_date: '2026-11-13 15:13'
labels:
  - testing
  - ci
dependencies: []
parent_task_id: TASK-330
priority: medium
ordinal: 741000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-330.1's .github/workflows/sandboxes.yml runs scripts/sandboxes.sh on GitHub's ubuntu-24.04 runner (4 vCPUs, 16 GB, 3 GB swap). The short mode (8 VMs at once, ~3-5 min) runs whole there. The long mode never has: 16 VMs at once, 10.75 and then 9 GiB of guests: 'The runner has received a shutdown signal' 7-7.5 min in (vmstat: free under 200 MB, swap 3 GB full); 8 at once of 32 with daemons of 4 GiB: down 6 min in, the RSS log showing each daemon's extent cache climbing to 2.9 GB beside 2.2 GB builders; daemons of 2 GiB: ran whole (12.5 min) but DAX windows refused (ENOMEM, no room to pin their private pages); daemons of 3 GiB, windows halved (main's sizes now): the job ended 66 min in with its run step never finished and no log uploaded (run 42944951179). So the nightly doesn't call it (nightly.yml unchanged). Options: a larger runner (GitHub's 8- or 16-core, paid), a self-hosted one, or a long mode sized for 16 GB (fewer at once, daemons' caches bounded lower with fewer windows), measured with the workflow's vmstat and RSS log. Done when the nightly runs the long mode whole.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 scripts/sandboxes.sh long finishes on ubu1 under fxjob --bench in about an hour at LangSmith's shape, and passes on main
- [x] #2 The README's testing section and TASK-330.4's pre-pin procedure say to run it on ubu1 before each pin; nightly.yml does not call it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Instrument: scripts/lib/resources.sh (host and cgroup memory, CPU, PSI, top processes every 30 s into results/resources.log), sourced by scripts/sandboxes.sh; the case says each VM's boot time and run time.
2. Run main's long mode on ubu1 under fxjob --bench (worktree /mnt/fxdax-scratch/t3305, artifacts built once, cached release kernel) and read where the time went: per role, per VM, CPU, memory pressure, I/O, other agents' jobs.
3. Size it from that to about an hour at LangSmith's shape (2 hosts, 8 VMs at once, 8-vCPU builders, overcommitted), without loosening any check; verify on ubu1.
4. The long mode leaves CI: sandboxes.yml runs short alone; README's testing section says the long one runs on ubu1 by hand before each pin (TASK-330.4's procedure).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On ubu1 too (main's sizes: 8 at once of 32, daemons 3 GiB; fxjob --bench, 8 CPUs, 16 GB cap): after 2 h 10 min only 5 of 32 VMs had ended, 15 booted, the scope at 15.1 of 16 GB, 57,000 CPU-seconds; stopped. So the long mode as sized (3 builds of 512 MiB a builder, pgbench at scale 50 x 3 runs of 60 s, git 3,000 files x 100 commits, 32 VMs) is past what 16 GB and 4-8 CPUs finish in hours: size it down (fewer VMs in all, smaller pgbench) and measure its time and memory on ubu1 before the nightly takes it.

Decided by the user, 2026-11-11: the long mode is not a nightly CI job. Nobody pays for larger runners, and it isn't shrunk to fit a 16 GB runner. It runs on ubu1 under fxjob --bench, by hand, before each pin handed to LangSmith, as part of TASK-330.4's pre-pin qualification, and when that work warrants it.

Still to do: it ran over 2 hours on ubu1 without finishing. Find out why (contention with other agents' jobs, a stuck VM, or simply too much work) and size it to finish in about an hour on ubu1 at full LangSmith shape. Document it in the README's testing section and in TASK-330.4's procedure.

2026-11-11 (claude): why it never finished: memory, not work or a stuck VM. Run on ubu1 under fxjob --bench (8 CPUs) with main's sizes and the new resources.log: the run's cgroup sat at its 16 GiB max (28,547 memory.max events) and the kernel OOM-killed an extentd 3-4 min in (memcg of the run): guests 7.5 GiB at once, each daemon ~4 GB RSS (3.1 GB shmem cache + 0.8 GB anon), RustFS ~1 GB, page cache; one builder's DAX window refused (ENOMEM). The 2 h+ run at 15.1 of 16 GB was this pressure without the kill: reclaim and compaction (kcompactd 42 CPU-min host-wide). At 20 GiB the job was killed again, by fxdax.slice's own 18 GiB max (every fxdax job's; a measurement has 16 GiB at most, system-setup.d/146). Not contention: other agents' jobs were on their own CPUs; RustFS, in Docker, ran outside the run's cgroup and CPUs (now a process inside it, user's decision).
Sized to fit 16 GiB at LangSmith's shape (2 hosts, 16 VMs 8 at a time, 8-vCPU builders, 28 vCPUs on 8 CPUs): the long mode's own VM sizes (builder 1 GiB, pgbench 768 MiB, git and writer 512 MiB: 5.5 GiB at once), daemons 2 GiB, DAX windows half the short mode's (root 512 MiB, own stores 256 MiB: at the short mode's, daemons of 2 GiB refused 2 windows a host, ENOMEM). Run 4 (ubu1, fxjob --bench --mem 16G): every VM did its role, VMs 807 s (builders 670-795 s for 3 builds of 512 MiB, pgbench 551-652 s, git 64-123 s, writers 45-62 s), no OOM (cgroup at its max, reclaiming), budgets ok (anon 991 MiB of 3,078), stores 1.20x the guests' bytes, daemons 48.8/48.6 GB written to disk; failed only the framing check (60/59 framings past 1 s, slowest 14.9/7.4 s), TASK-310's remainder. ~14 min, not an hour: at its write rate (the daemons ~97 GB to disk in 14 min) an hour's bucket, never collected in a run, would pass the 251 GB scratch disk.

2026-11-12 (claude): in the qualification A/B's clean rounds (main 0ee1bdb58, TASK-310 in), the long mode ran whole in 798 s of VMs (~15 min with setup) within 16 GiB, every VM its role but two git VMs lost to a harness race (fixed), and failed only the framing check (4-12 a host past 1 s, slowest 1.2-4.7 s). AC #1's 'passes on main' waits on TASK-310's remainder; AC #2 (README, procedure, no nightly) is done.

2026-11-13 (claude): one long-mode run on main 4e0bf82f5 (TASK-369/384 in, TASK-382, TASK-388), ubu1 under fxjob --bench (16 GiB), qualify.sh's sandboxes part (run q3305, main-r1, 08:21:31-~08:36 UTC): PASSED, no failed check. VMs 794.4 s (builders 766 s, pgbench 634, git 160, writers 44). Framings: none past 1 s (slowest 685.6 / 949.3 ms a host; 673/658 logged as slow, sum 86.5/87.4 s); stores 1.21x the guests' 73.9 GiB (host-a 1.21, host-b 1.21), daemons' disk 1.21x; no memory-guard stop, daemons' peak RSS 3,076/3,058 MiB, anon 993/969 MiB, every daemon budget (memory, uploads, disk) ok; daemon CPU 528.9 + 580.1 s = 14.66 ms per guest MiB against the 20 ms budget (27% headroom; qualify-budgets.py over its budgets.json); commands slowest 97 ms. Both daemons' durable lag over every VM's commits: within 50 ms 61.3% (47,261/77,037) and 64.7% (46,553/71,910), p50 bucket <=64 ms, p99 <=256 ms, max 1.72/3.37 s. Clean of disk contamination: zd0's writes not the bench's 0.5-6.3 MB/s and fxdax-work.slice's ~0 through it (30 s samples). The soak (part 2) and the second round were stopped by the coordinator mid-soak (ubu1 overloaded: the run went past the 15 minutes agreed with the user). So framings pass once; one run closes nothing: a second long-mode pass waits for the user, by the coordinator. Not done: the soak's bucket-faults and soak CPU budget under bench. GitHub's short mode on the same main (run 43390653591) still fails framings, 2 a host past 1 s, every one write phase (TASK-336), so ci.yml can't take the short mode yet.

2026-11-13 (claude): second long-mode pass, main 10f613290 (soak off), ubu1 fxjob --bench --mem 16G, 11:15:17-11:28:28 UTC: FAILED the framing check alone: host-b framed 1 segment in 1,045.0 ms (own 0.0, write 1,045.0: all write phase, no sync), host-a's slowest 908.3 ms (685/685 logged slow, sum 94.1/90.1 s). Everything else passed: VMs 755.5 s, stores 1.21x the guests' 73.8 GiB, no memory-guard stop (peak RSS 3.22/3.32 GB, anon 1,010 MiB at most), daemon CPU 14.47 ms per guest MiB of 20 (28% headroom), commands <= 86 ms; durable lag 59.1%/61.0% within 50 ms, p50 <=64 ms, p99 <=512 ms, max 4.40/2.32 s. Clean: zd0's writes not the bench's at most 12.1 MB/s (mostly 1-2), fxdax-work.slice's at most 1 MB/s. The run's cgroup CPU pressure (some avg10) reached 65%: 28 vCPUs on 8 CPUs starve the framer's write phase, TASK-336's cause, now on ubu1 too. So the framing check is marginal on main under bench: slowest 949 ms (pass 1) and 1,045 ms (pass 2). AC #1 not met; open until TASK-336.

2026-11-13 (claude): the soak under fxjob --bench, main 10f613290, 11:29:09-11:59:15 UTC (qualify.sh's soak part alone, 1,800 s, RustFS through the faulty proxy): PASSED, clean (zd0's other writes at most 1.4 MB/s, work 0). Bucket faults said: 73 in the full phase, 22 more out of service (95 in all), none answering or slow. Daemon CPU 5.45 ms per guest MiB of the soak's 20 (84.2 s), anon 1,630 of 2,947 MiB, uploads 1,533 of 1,536 MiB, disk 15.2 GiB for the guests' 15,360 MiB (1.008x), stores uploaded 14.6-20.4 s after the bucket answered, longest fsync 251 s (the outage). TASK-382's soak budget confirmed under bench.

2026-11-13 (claude, TASK-336's agent): long mode on ubu1 (fxjob --bench --mem 16G), afternoon, three passes, all failing the framing check alone on the disk: main 487f443a8 5 past 1 s (slowest 1.65 s); main c6ba0d117 19 (slowest 5.77 s; read phases of raw files evicted under the cgroup, TASK-400, and write-back waits, TASK-401); main before TASK-336 (c211ff96) 11 (slowest 2.97 s). Durable lag within 50 ms 36-60%, max 5.1-12.6 s, against this morning's 59-65% and 1.7-4.4 s: ubu1's disk degraded through the day (the same build's short mode 72% -> 36-46% within 50 ms, its I/O stall 3 -> 9-27 s; host swap full, other users' apps resident), so passes hours apart don't compare; compare within ABBA rounds. Everything else passed each time: stores 1.20-1.21x, daemon CPU 14.3-15.3 ms per guest MiB of 20, commands < 100 ms, no memory-guard stop. AC #1 waits on TASK-400 and TASK-401.
<!-- SECTION:NOTES:END -->
