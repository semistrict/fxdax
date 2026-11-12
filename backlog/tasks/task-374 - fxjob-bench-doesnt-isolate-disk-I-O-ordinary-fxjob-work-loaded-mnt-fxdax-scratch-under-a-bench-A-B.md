---
id: TASK-374
title: >-
  fxjob --bench doesn't isolate disk I/O: ordinary fxjob work loaded
  /mnt/fxdax-scratch under a bench A/B
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-12 07:45'
updated_date: '2026-11-12 08:06'
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
- [ ] #1 A disk-heavy ordinary fxjob running beside an fxjob --bench job is limited (or queued) while the bench job runs, shown by iostat or the cgroup's io.stat
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
<!-- SECTION:NOTES:END -->
