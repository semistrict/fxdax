---
id: TASK-350.2
title: 'Bench: random reads over a working set larger than the daemon''s memory'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 11:01'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 706000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With DAX the daemon's memory is the guest's only file cache, and a page-in decompresses a whole 1 MiB zstd part (crates/fxdax/src/cache.rs). The sandboxes' guests cache files in their own RAM, a miss a 4 KiB read of an uncompressed block file. A Linux tree's warm reread took 13.15 s with 1 GiB of daemon memory against 0.6 s with 8 GiB (README, Sizing).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Random 4 KiB reads (fio, and a database doing point lookups) over working sets of 0.5x, 1x, 2x and 4x the tenant's daemon memory, guest RAM equal in both cases, fxdax with DAX and without, against the loop-device baseline (TASK-350.1)
- [ ] #2 Reports IOPS, p50/p99 latency, and per miss: bytes decompressed and host CPU
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
