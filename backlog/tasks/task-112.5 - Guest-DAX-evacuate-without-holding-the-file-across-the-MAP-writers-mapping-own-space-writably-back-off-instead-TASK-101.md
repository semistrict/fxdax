---
id: TASK-112.5
title: >-
  Guest DAX: evacuate without holding the file across the MAP; writers mapping
  own space writably back off instead (TASK-101)
status: To Do
assignee: []
created_date: '2026-10-28 04:48'
updated_date: '2026-10-28 14:13'
labels: []
dependencies: []
parent_task_id: TASK-112
priority: high
ordinal: 428000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
After each seal, fxfs_dax_evacuate_file holds the file DAX semaphore for writing through a MAP_MANY round trip (~400 us of ~650): PostgreSQL WAL is evacuated after nearly each commit and its next write(2), which maps a new block and needs the semaphore for writing, waits ~390 us: 17.8 s of read and write waits per 60 s pgbench run at 2 GiB (TASK-112.4 counters). Measured 2026-10-28 (all on GCE n2 nested): (1) without evacuation at all, delay-injected checks pass 5/5 and pgbench is fast while region space lasts (bench VMs 1513 tps vs 1364 and ext4 dax=always on virtio-pmem 1496; 8 GiB run 1 1306 vs 1125) but collapses once it fills (2 GiB 667 -> 210: a staging region holds pages of many files, so reclaiming LRU ranges frees few regions); (2) batching evacuations (16 seals / 20 ms) +3-6% but delay-injected checks failed 2/8 (append_writeback zeros, random_ops mismatch) where main passes 5/5: delaying evacuation exposes a race not yet found; (3) holding the semaphore for reading through the MAP: readers stop waiting but WAL writers still wait (7.5 s), tps unchanged; its placing wait deadlocked (fxfs_dax_released fires only when users reach zero); (4) dropping instead of mapping is wrong: a range mapped bit means the file has data there, so pages became holes. Design left: gather and place under the write lock, MAP with no lock; a writer that would map a page in a held range own space writably (map_own, MAP_MANY own pieces, e->open) backs off, releasing the semaphore and waiting for the hold, because the evacuation MAP landing after it would make its pages read-only (TASK-101); writers moving pages to the staging region (the normal case) do not conflict. Mind TASK-79.106 (map_extents partial failure).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An evacuation holds no file semaphore while its MAP is in flight
- [ ] #2 A writer that maps a page of a range an evacuation holds in its own space writably waits for the evacuation; a test shows it (with fxfs.dax_delay_us)
- [ ] #3 fxfs-guest-checks-dax under fxfs.dax_delay_us=200 passes 5 rounds
- [ ] #4 pgbench-dax 2 GiB: dax_read_wait_ns + dax_write_wait_ns per run under 2 s; tps recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-28 attempt (branch wip/task-112-5, unpushed): gather and place under the semaphore, MAP without it; own-space MAPs wait for an evacuation's in flight (fxfs_dax_wait_evacuee; check evacuee_waited, fault injection fxfs.dax_evacuee_delay_ms, AC2 shown 5/5); placing waits for the ranges' other users. GCE r205: DAX suites green, pgbench-dax 1216/1105/1044/1018/1017 vs 1172/1085/1022/983/983 on main (same host, +2-3.5%), dax_write_wait_ns 72 ms total. Not landed: under fxfs.dax_delay_us=200 random_ops loses data ~1 round in 6 (TASK-112.6's race, which any lateness of the evacuation exposes).
<!-- SECTION:NOTES:END -->
