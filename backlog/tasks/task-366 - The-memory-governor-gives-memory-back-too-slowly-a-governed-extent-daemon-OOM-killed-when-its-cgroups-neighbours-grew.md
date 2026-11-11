---
id: TASK-366
title: >-
  The memory governor gives memory back too slowly: a governed extent daemon
  OOM-killed when its cgroup's neighbours grew
status: To Do
assignee: []
created_date: '2026-11-11 21:24'
labels:
  - bug
  - extentd
dependencies: []
priority: high
ordinal: 750000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found on ubu1, 2026-11-11 (journalctl -k): two governed fxdax-extentd daemons in fxjob --bench jobs were OOM-killed. 15:20 local: oom_memcg was the job's own scope (fxjob-t3305-long, the daemon at 802 MB anon); 15:44: oom_memcg was fxdax.slice (then MemoryMax 18G), task fxjob-t3305-long2's fxdax-extentd, while work jobs grew beside it. TASK-358's governor (crates/fxdax/src/extentd/governor.rs) looks every 100 ms at MemAvailable and each ancestor cgroup's room (max + inactive_file - current), and evicts past the allowance; its memory is memfd, which reclaim can't take, so a neighbour growing faster than one look per 100 ms, or past the headroom between looks, makes the kernel OOM-kill instead. Expected: the daemon gives memory back before any cgroup it is in reaches its limit; a governed daemon is never OOM-killed for memory it holds as cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A test reproduces it: a governed daemon holding its allowance, and a neighbour in a shared parent cgroup allocating fast, and the daemon is not OOM-killed
- [ ] #2 The governor reacts to pressure as it comes (memory.events high/max, PSI triggers on memory.pressure, or a memory.high of its own below the max), not only on a 100 ms poll
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
