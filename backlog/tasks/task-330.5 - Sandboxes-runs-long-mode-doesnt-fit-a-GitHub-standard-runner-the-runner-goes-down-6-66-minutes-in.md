---
id: TASK-330.5
title: >-
  Sandboxes run's long mode runs on ubu1 by hand before each pin, not nightly in
  CI; it must finish there (it ran 2+ h unfinished)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-11 07:25'
updated_date: '2026-11-12 11:35'
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
<!-- SECTION:NOTES:END -->
