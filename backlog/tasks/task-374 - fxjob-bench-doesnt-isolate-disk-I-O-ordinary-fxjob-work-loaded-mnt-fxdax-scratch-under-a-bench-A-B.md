---
id: TASK-374
title: >-
  fxjob --bench doesn't isolate disk I/O: ordinary fxjob work loaded
  /mnt/fxdax-scratch under a bench A/B
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 07:45'
updated_date: '2026-11-12 09:32'
labels:
  - testing
  - bug
dependencies: []
priority: medium
ordinal: 756000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (2026-11-12), TASK-369's disk loader (loader.sh: dd of 1 GiB random with conv=fsync in a loop to /mnt/fxdax-scratch) ran as ordinary fxjob work from 01:54 EST, beside TASK-330.4's fxjob --bench A/B (run-r3305) on the same NVMe scratch disk, at about 186 MB/s of writes. fxjob-isolate (system-setup.d/146) partitions CPUs, L3 ways and memory bandwidth, and caps others' clocks, but nothing limits fxdax-work.slice's disk I/O, so the bench's durable-lag and framing numbers were measured under someone else's disk load without any warning. Give fxdax-work.slice an io limit (io.weight, or io.max on the scratch device's major:minor) while a bench job runs, as the isolation already does for MBA, or refuse/queue disk-heavy jobs then; and have bench-ab.py record any other job's disk throughput during its rounds.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A disk-heavy ordinary fxjob running beside an fxjob --bench job is limited (or queued) while the bench job runs, shown by iostat or the cgroup's io.stat
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fxjob --bench caps fxdax-work.slice's IO on /mnt/fxdax-scratch (io.max through the user's manager: 64M/2000 read, 32M/1000 write) after isolating, and lifts it in its EXIT trap; fails if the scratch disk isn't mounted. fxjob ps shows each slice's io.max.
2. Rolled out with TASK-353 in one run of step 146.
3. Prove: a 256 MiB dd conv=fsync loop as work beside a short fxjob --bench queued behind run-r3305; loader rates and io.stat before, during and after.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
AC #2 (bench rounds recording other jobs' disk I/O) moved to TASK-375, filed the same hour by the TASK-330.4 agent; this task is the fxjob-side limit only.

fxjob --bench now caps fxdax-work.slice's IO on /mnt/fxdax-scratch once isolated (systemctl --user set-property --runtime: IOReadBandwidthMax 64M, IOWriteBandwidthMax 32M, IOReadIOPSMax 2000, IOWriteIOPSMax 1000; io.max on 230:0, the zvol, which has no IO scheduler for io.weight to reach) and lifts it in its EXIT trap; refuses to run if the scratch disk isn't mounted; fxjob ps shows each slice's io.max. Rolled out with TASK-353 in the same run of step 146 (02:56 EST).
Mechanism checks (own throwaway slice under fxdax.slice): buffered dd + fsync 256 MiB capped at 32M ran at 29-31 MB/s against 264-356 MB/s uncapped; reads at 64M ran 59 MB/s. A cap on a fresh scope itself was unreliable (its writes were sometimes attributed to the parent slice), so the cap sits on the slice.
Proof, ubu1 04:31 EST, queued behind run-r3305: a 256 MiB dd conv=fsync loop as ordinary work started as the bench job started: 26, 27, 27 MB/s while the bench held the slot (work's io.stat: 768 MiB in 30.4 s, 25 MB/s); the bench's exit lifted it (io.max empty) and the next 256 MiB ran at 392 MB/s. Before the cap, the same dd ran 111 MB/s to 1.3 GB/s, and TASK-369's loader 186 MB/s.
Caveat found in the proof: that last unlimited dd (04:31:34.03-34.71, scope gone 34.77) overlapped by 0.3 s the start of another agent's round, fxjob-rounds-bench-fs-fxfs-guest-dax-unsafe-1 (started 04:31:34.48, queued behind the proof's bench). The proof's loader no longer exists; nothing of it runs again.

DoD #1 not run: no Rust changed (fxjob lives in ubu1's step 146).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxjob --bench caps ordinary work's IO on the scratch disk (io.max on fxdax-work.slice, 64 MB/s/2000 IOPS read, 32 MB/s/1000 IOPS write) while it holds the slot, and lifts it at exit; rolled out with TASK-353 in one run of step 146 that replaced only fxjob. Proven on ubu1: a dd+fsync loop in work ran 26-27 MB/s beside a bench job (io.stat: 25 MB/s), 392 MB/s once it ended.
<!-- SECTION:FINAL_SUMMARY:END -->
