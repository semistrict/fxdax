---
id: TASK-374
title: >-
  fxjob --bench doesn't isolate disk I/O: ordinary fxjob work loaded
  /mnt/fxdax-scratch under a bench A/B
status: To Do
assignee: []
created_date: '2026-11-12 07:45'
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
- [ ] #2 A bench round run while another job writes heavily to its disk says so in its results
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
