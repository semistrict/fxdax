---
id: TASK-28.11
title: 'GCE: branches on GCS end to end, and read throughput'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 01:32'
updated_date: '2026-10-18 19:16'
labels: []
milestone: m-5
dependencies:
  - TASK-28.9
  - TASK-28.10
documentation:
  - >-
    backlog/docs/doc-7 -
    Object-storage-how-the-host-keeps-branches-in-S3-and-GCS.md
parent_task_id: TASK-28
type: task
ordinal: 91000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-7 decision 9 and Testing: perf and real-store behaviour are measured on GCE only. The part size (256 KiB to 4 MiB) and GETs in flight are chosen by measurement.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The guest suites pass with branches on GCS
- [ ] #2 A VM resumes on a second GCE host from GCS and its filesystem is intact
- [ ] #3 Ranged-GET throughput per host and the part-size sweep are recorded in docs/measurements, with the chosen settings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. test-gce.sh: FXDAX_EXTENTD_REMOTE (gs://) forwarded, under a prefix of the run's own; run the guest suites with branches on GCS.
2. Snapshot test Migrate mode: FXDAX_MIGRATE_PHASE source/target with a bundle (snapshot, memory, rootfs); the bucket from the environment. scripts/migrate-gce.sh: two c3 hosts, source on one, bundle through GCS, target on the other.
3. fxdax-gets: ranged-GET throughput by part size and GETs in flight; scripts/gets-gce.sh records it in docs/measurements, with the chosen settings.
<!-- SECTION:PLAN:END -->
