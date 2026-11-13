---
id: TASK-400
title: >-
  Framings read raw files back from the disk once the host evicted them: long
  mode's slowest framings are their read phase, 0.7-5.7 s, waiting on a
  saturated disk
status: To Do
assignee: []
created_date: '2026-11-13 15:12'
labels:
  - perf
  - extentd
dependencies: []
priority: high
type: bug
ordinal: 783000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by TASK-336's instrumentation (read / compress / write phases, runnable waits). sandboxes long mode on ubu1 (fxjob --bench --mem 16G, its cgroup at its 16 GiB limit, reclaiming), main c6ba0d117 (2026-11-13): 19 framings past 1 s, the slowest of them their read phase, 0.7-5.7 s with 0-50 ms runnable: the segment's raw file, written by its guest moments before (and written back by write-behind, TASK-221), was evicted from the page cache under the cgroup's memory pressure and is read back from a disk eight VMs and RustFS keep busy. Main before TASK-336 in the same conditions (same afternoon) failed 11, the slowest 3.0 s, read-bound too. Ideas, unmeasured: frame (or start framing: checksum, sample, compress parts) while the segment's data is still in memory, part by part as it fills, so closing has only the tail left; or keep a closed segment's raw pages resident until framed; or size the long mode's cgroup and daemons' caches so the page cache keeps the last segments. Measure with the framing warnings' read phase and runnable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Long mode on ubu1 under fxjob --bench: no framing's read phase past 0.5 s, over two passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
