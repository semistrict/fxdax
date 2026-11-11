---
id: TASK-350.3
title: 'Bench: cold sequential scans of data on local disk'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 10:59'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 707000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Cold DAX reads ran 453 MiB/s against 3,325 MiB/s for ext4 on virtio-blk, sqlite-pread-scan 284 against 2,617 MiB/s (docs/measurements/gce-fs-bench-2026-10-31-refresh); without DAX random cold reads fell to 3,173 IOPS. The baseline's path for the same data is a virtio-blk image file read through JuiceFS's FUSE mount from its local block cache, never measured against it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequential reads of a multi-GiB file, a tar of a large tree and a SQLite table scan, cold (host page cache, daemon cache and guest page cache dropped; data in the local part cache / JuiceFS block cache), fxdax with and without DAX against the JuiceFS baseline
- [ ] #2 Also with the data only in the bucket (part cache and JuiceFS cache emptied)
- [ ] #3 Reports MiB/s and host CPU per GiB read
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
