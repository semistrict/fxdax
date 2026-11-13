---
id: TASK-396
title: >-
  fxjob admits jobs by what they reserve: a memory and CPU budget, a
  first-come-first-served queue, history (ubu1)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:58:00Z'
updated_date: '2026-11-13T08:58:00Z'
labels:
  - ubu1
dependencies: []
priority: high
ordinal: 771000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-13: agents start fxjob jobs with nothing comparing what they ask for to what the host has: load ~70 on 8 cores, swap full, two --mem 16G jobs and a cargo build at once. Jenkins, k8s and slot-only tools (task-spooler, nq, sem) were ruled out, Slurm is the fallback; the decision is to extend fxjob. Reservations are the live scopes (MemoryMax, cpus= in the description), the budget /etc/fxjob.conf, the queue and lock in $XDG_RUNTIME_DIR/fxjob, history ~/.local/state/fxjob/jobs.jsonl. fxjob had no copy in the repo: its source moves to scripts/fxjob, installed as /usr/local/bin/fxjob by ~/docs/system-setup.d/146-fxdax_cgroups.sh.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A job waits until its --mem and --cpus fit beside running jobs within MEM_BUDGET and CPU_BUDGET, first come first served
- [x] #2 A request past the whole budget fails at once with status 2
- [x] #3 A job whose processes are killed (kill -9) frees its reservation; a waiting fxjob killed frees its place
- [x] #4 A measurement (--bench) reserves its 8 CPUs and memory, waits for work to drain, keeps its lock and isolation
- [x] #5 Each job that ran appends a JSON line: name, mem, cpus, peak memory and swap, CPU seconds, queued and wall time, status
- [x] #6 fxjob ps lists waiting jobs and reserved vs budget, with no cpuset errors (TASK-376)
- [x] #7 scripts/fxjob-check.sh passes on ubu1 against the worktree copy
- [ ] #8 Installed on ubu1 as /usr/local/bin/fxjob, step 146 installing it from the repo
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
scripts/fxjob-check.sh: 41 checks pass on ubu1 (2026-11-13), run in a scope of MemoryMax=1G, no swap, its jobs in fxdax-fxjobcheck*.slice on a 300M/10 CPU budget, isolation off. --cpus is recorded in the scope description (cpus=N), not CPUQuota/CPUWeight: either would enable the cpu controller in fxdax.slice, whose subtree has only io memory pids, and share CPU by cgroup instead of by the niceness host builds run at. CPUs count logical CPUs (budget defaults to nproc, 16; a measurement reserves its 8, 4-7,12-15; --cpus defaults to 4, FXDAX_HOST_JOBS). Peaks are read by a shell inside the scope once the command ends: systemd forgets a scope (MemoryPeak [not set]) as soon as it ends. A nested fxjob (inside a job scope) skips the queue, as queueing it could deadlock with its parent. An incident while testing: an earlier OOM case, tail /dev/zero, streamed zeros into the driver pipe's tail -70 (outside any job) which grew to 11.7 GB until user-1000.slice OOM-killed it; nothing else was killed. The check now allocates a bounded 256M and the driver runs capped. Left: install (sudo) and change step 146 to install scripts/fxjob instead of its heredoc; host-remote.sh could pass --cpus FXDAX_HOST_JOBS once installed.
<!-- SECTION:NOTES:END -->
