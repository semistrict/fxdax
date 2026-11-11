---
id: TASK-350.3
title: 'Bench: cold sequential scans of data on local disk'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 13:47'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 707000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cold DAX reads ran 453 MiB/s against 3,325 MiB/s for ext4 on virtio-blk, sqlite-pread-scan 284 against 2,617 MiB/s (docs/measurements/gce-fs-bench-2026-10-31-refresh); without DAX random cold reads fell to 3,173 IOPS.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequential reads of a multi-GiB file, a tar of a large tree and a SQLite table scan, cold (host caches, daemon cache and guest page cache dropped; fxdax's data in its local part cache), fxdax with and without DAX against the loop-device baseline (TASK-350.1)
- [ ] #2 Reports MiB/s and host CPU per GiB read
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
