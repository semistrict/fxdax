---
id: TASK-336
title: >-
  Framings' write phase takes over a second on a CPU-overcommitted host:
  extentd's framing competes with vCPUs (GitHub runner, 28 vCPUs on 4 CPUs)
status: To Do
assignee: []
created_date: '2026-11-10 23:37'
updated_date: '2026-11-13 08:20'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 737000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-330.1's sandboxes run on GitHub's ubuntu-24.04 runner (4 AMD EPYC vCPUs under Hyper-V, ext4 root mounted nobarrier, data=writeback), short mode: 8 VMs at once, 28 vCPUs, two extent daemons and RustFS on the disk (probe run on branch t330-probe, 2026-11-10). Slowest framings per host 1,159 and 1,292 ms; of the four past 1 s, two are their write phase alone: 'took 1079.1 ms (own 0.0, write 1078.0, sync 1.1, dirs 0.0 ms)' and 'took 1042.3 ms (write 1039.3, sync 2.1)'; the others mostly sync (1,175 ms; 724 ms + write 430 ms). The write phase reads the segment, samples its parts' compression, compresses if it must, and writes the tail: CPU, which the host's vCPU threads take first. On ubu1 with 8 dedicated CPUs (fxjob --bench, same 28 vCPUs) the write phase stays under 0.2 s and slow framings are the syncs (TASK-310's remainder). LangSmith's runners have this shape (8-vCPU VMs on an 8-CPU runner). A framing past a second holds every commit behind it as long. Ideas, unmeasured: run framings at a higher scheduling priority than VM processes (or VMs' threads at a lower), bound framing CPU by sampling less, pin the daemon's framing threads; or show it doesn't matter to commits (they wait on framing's sync, not its write).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 On the sandboxes run's short mode on a 4-CPU GitHub runner, no framing's write phase passes 1 s, shown over several runs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-13 (claude, TASK-330.5's re-run): GitHub's runner, sandboxes short mode on main 4e0bf82f5 (TASK-369/384/382/388 in; run 43390653591): fails only framings, 2 a host past 1 s (host-a 1,363.0 and 1,264.0 ms, host-b 1,416.3 and 1,357.1 ms), every one of them all write phase (own 0.0, write = total): the syncs no longer show at all. 100/97 framings past the logging threshold, sum 20.8/21.3 s; stores 1.20/1.19x; commands <= 117 ms; daemons 1.73/1.78 GB peak RSS, 33.8/33.3 s CPU; VMs 147.9 s. So this task is now what keeps the short mode out of ci.yml on GitHub's 4-CPU runners.
<!-- SECTION:NOTES:END -->
