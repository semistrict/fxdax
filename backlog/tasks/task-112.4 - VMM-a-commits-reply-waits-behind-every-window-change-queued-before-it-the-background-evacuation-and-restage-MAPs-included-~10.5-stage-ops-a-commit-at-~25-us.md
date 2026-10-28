---
id: TASK-112.4
title: >-
  Guest DAX: evacuation holds each file's DAX semaphore for writing ~650 us, its
  MAP inside, 32 s of a 60 s pgbench run
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-28 03:23'
updated_date: '2026-10-28 05:03'
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
- [ ] #1 The time reads, writes and faults wait for a file semaphore an evacuation holds is counted and reported in /proc/fs/fxfs
- [ ] #2 The evacuation MAP is made without the file semaphore held for writing, or the wait counted is shown to be small
- [ ] #3 fxfs-guest-pgbench-dax 2 GiB on GCE: tps against ff7961411 recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count reads' and faults' waits for the file semaphore (dax_read_waits/_ns, dax_fault_waits/_ns).
2. Found (r203, 2 GiB, run 1): 45.7k read waits, 17.8 s, ~390 us each, about one a transaction; faults 0; writes mapping anew 5k, 1.6 ms.
3. Tried: hold the semaphore for reading through the MAP (downgrade). Write holds 32.7 -> 15.6 s, read waits 17.8 -> 5.3 s, but writes mapping anew (the WAL's) waited 7.5 s (from 1.6 ms); tps unchanged (1163 run 1). It also hung fxfs-guest-checks-dax under fxfs.dax_delay_us=200: the placing waited for range users to drop to one, which no put woke (released counts drops to zero). Reverted.
4. Fix: batch evacuations: one per 16 seals, or 20 ms after the first seal of a batch (delayed work), so files are held ~10x less often, each MAP_MANY carrying a batch's runs.
5. Verify: pgbench-dax 2 GiB tps and waits; DAX suites; checks under fxfs.dax_delay_us=200 x3.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r203-112w (counters, before the fix): tps 1181/1053/1016/970/989; run 1 dax_read_waits 45,698 (17.83 s), dax_fault_waits 0, dax_write_waits 4,968 (1.6 ms), evacuations 54,607 holding 32.66 s, stores_held_by_seals 11,577 (4.97 s).

2026-10-28 night: none of the evacuation changes landed. Batching (16 seals / 20 ms): +3-6% tps but delay-injected checks failed 2/8 (TASK-112.6). Semaphore downgraded for the MAP: writers (the WAL's) still waited (7.5 s), and the placing wait deadlocked. No evacuation (TASK-109.1): fast until region space fills, then 667 -> 210 tps. Evacuating by dropping instead of mapping: pages became holes (mapped means data). The read/fault wait counters were not landed either: the tree carrying them soft-locked once in faults_seal_holding_the_semaphore under fxfs.dax_delay_us (TASK-113), not shown to be theirs. The fix left is TASK-112.5.
<!-- SECTION:NOTES:END -->
