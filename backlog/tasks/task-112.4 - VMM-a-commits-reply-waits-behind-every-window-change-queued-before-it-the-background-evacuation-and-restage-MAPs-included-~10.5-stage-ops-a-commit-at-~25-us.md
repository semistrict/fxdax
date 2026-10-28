---
id: TASK-112.4
title: >-
  Guest DAX: evacuation holds each file's DAX semaphore for writing ~650 us, its
  MAP inside, 32 s of a 60 s pgbench run
status: To Do
assignee: []
created_date: '2026-10-28 03:23'
updated_date: '2026-10-28 03:24'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 427000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-9: fxfs_dax_evacuate_file takes the file semaphore for writing, gathers the pages a seal sealed in the staging region, MAPs them in the ranges own space (a MAP_MANY round trip, ~400 us) and places them, then lets go: 49.7k evacuations, 32.4 s of holds per 60 s run at 2 GiB (dax_evacuate_hold_ns). Reads, writes and faults of the file wait meanwhile, and PostgreSQL WAL is evacuated after nearly every commit. How long they wait is not counted yet.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A commit reply waits only for the window changes it ordered (its write-protects), not for other requests changes to other window ranges
- [ ] #2 A test shows a commit reply delivered while an unrelated MAP stage change is still being applied
- [ ] #3 fxfs-guest-pgbench-dax on GCE: guest device_commits_ns per commit falls, tps recorded against ff7961411
- [ ] #4 The time reads, writes and faults wait for a file semaphore an evacuation holds is counted and reported in /proc/fs/fxfs
- [ ] #5 The evacuation MAP is made without the file semaphore held for writing, or the wait counted is shown to be small
- [ ] #6 fxfs-guest-pgbench-dax 2 GiB on GCE: tps against ff7961411 recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
