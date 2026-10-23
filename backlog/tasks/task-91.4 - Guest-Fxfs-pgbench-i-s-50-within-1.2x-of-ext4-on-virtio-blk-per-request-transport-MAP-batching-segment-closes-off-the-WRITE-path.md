---
id: TASK-91.4
title: >-
  Guest Fxfs: pgbench -i -s 50 within 1.2x of ext4 on virtio-blk: per-request
  transport, MAP batching, segment closes off the WRITE path
status: In Progress
assignee:
  - '@main-session'
created_date: '2026-10-22 18:22'
updated_date: '2026-10-23 22:12'
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Load step with DAX (pgbench -i -s 50, GCE, /proc/fs/fxfs counters, ba..): ~40,300 MAPs taking ~10.8 s of the 23 s step. By why: 32,399 pieces mapped in ranges' own space (974 MB, ~30 KB each), each after the regions' half of the window had no room (33,496 'full'); 5,227 background grows and 2,604 reaches (2 GB together); 94 restages. Tried: reclaiming ranges that hold regions in the background when the regions' space ran low: the load fell to 19.6 s (MAPs 10,923, 405 in ranges) but the runs collapsed again (897 / 563 / 55 tps, regions to 20,000, unmaps ~58k a run): reverted. The underlying issue: under fsync-heavy load every seal leaves a region (a staging extent's), file ranges slice them and keep them, and they crowd the window; regions save MAPs when an extent is written in bulk, not when it holds a few pages of an fsync's epoch. A design question: e.g. map pieces of sealed, small staging extents in ranges' space rather than in regions, or regions per bulk writer only.

06dd6a64 + firecracker 1fbc852f9: MAP_MANY (doc-4). DAX pgbench: 388,693 maps in 375,084 requests (3.5% fewer): pgbench's writes map one piece at a time; load 28.8 s.
<!-- SECTION:NOTES:END -->
