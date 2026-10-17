---
id: TASK-28.11
title: 'GCE: branches on GCS end to end, and read throughput'
status: To Do
assignee: []
created_date: '2026-10-17 01:32'
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
