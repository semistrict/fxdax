---
id: TASK-375
title: >-
  A measurement round says when others wrote to its disk: fxjob --bench isolates
  CPU, cache and memory bandwidth, not the disk
status: To Do
assignee: []
created_date: '2026-11-12 07:45'
labels:
  - testing
  - ubu1
  - perf
dependencies: []
priority: high
ordinal: 757000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found 2026-11-12 by the coordinator, not by the run: TASK-330.4's qualification A/B ran round 1 wholly beside TASK-369's loader (ordinary work, 1 GiB rewritten with fsync in a loop, ~186 MB/s, on the same /mnt/fxdax-scratch NVMe), and nothing in the round said so; its framings and latencies looked like main's own. Every round of a measurement (scripts/qualify.sh, bench.sh through bench-ab-linux-host.sh, bench-host.sh) should record the device's writes and reads over the round (/sys/block/DEV/stat for the device under the round's disk) against its own cgroup's (io.stat), and the rest, others' I/O, in its results; past a bound (say 10 MB/s averaged, or any minute past 50 MB/s) the round is marked contaminated, and bench-ab.py leaves its timing metrics out of the comparison, saying so, as it fails a round a part didn't run in. resources.log samples the run's io.pressure already; it doesn't tell others' I/O from the run's own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A round run beside a disk loader is marked contaminated in its results and bench-ab.py's table leaves its timings out, saying so
- [ ] #2 A round run alone is not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
