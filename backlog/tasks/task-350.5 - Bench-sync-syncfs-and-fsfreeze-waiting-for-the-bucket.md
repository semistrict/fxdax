---
id: TASK-350.5
title: 'Bench: sync, syncfs and fsfreeze waiting for the bucket'
status: To Do
assignee: []
created_date: '2026-11-11 10:56'
updated_date: '2026-11-11 13:47'
labels: []
dependencies:
  - TASK-350.1
parent_task_id: TASK-350
priority: medium
ordinal: 709000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
sync, syncfs, unmount and freeze ask for COMMIT_DURABLE, which waits for the upload and a lease read: about 80 ms on GCS in region (docs/measurements/gcs-round-trips-2026-11-08.md). On a local disk they reach ext4's journal and a flush of the drive, an fsync of the image file with cache_type Writeback. append_writeback (fdatasyncs beside a syncfs loop) ran 3.3x slower on GCS than on a file bucket.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A loop of small writes each followed by sync, syncfs, and an fsfreeze/thaw cycle, fxdax at bucket latencies of 0, 25 and 80 ms, against the loop-device baseline (TASK-350.1)
- [ ] #2 A real workload that syncs (e.g. a docker build or a build script calling sync) timed end to end on both
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Baseline with cache_type Unsafe, as production (user, 2026-11-11): there sync, syncfs and freeze cost nothing beyond the guest.
<!-- SECTION:NOTES:END -->
