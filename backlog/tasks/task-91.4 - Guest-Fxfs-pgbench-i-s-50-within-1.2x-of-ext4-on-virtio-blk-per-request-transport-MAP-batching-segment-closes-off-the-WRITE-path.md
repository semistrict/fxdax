---
id: TASK-91.4
title: >-
  Guest Fxfs: pgbench -i -s 50 within 1.2x of ext4 on virtio-blk: per-request
  transport, MAP batching, segment closes off the WRITE path
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-23 10:16'
labels: []
dependencies: []
parent_task_id: TASK-91
priority: high
ordinal: 269000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The load takes 26 s on fxdax against 13.7 s on ext4 on virtio-blk, whatever the object-store latency. The guest makes about 95k synchronous device requests (MMIO exits to the VMM) against virtio-blk's 2.5k, halts 75k times between them, and spends 6.7 s more system time. The VMM saw 203 µs per MAP against extentd's 60 µs: about 140 µs of transport each. Postgres's backends block in fxfs_dax_iomap_write → fxfs_dax_map_extents → fxfs_device_map. Without DAX, WRITEs took 2.66 ms each with segment closes (6.6 s) on that path.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A guest CPU profile of the load step attributes the extra system and idle time
- [ ] #2 pgbench -i -s 50 within 1.2x of ext4 on virtio-blk on the same host (fs-bench, GCE)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
