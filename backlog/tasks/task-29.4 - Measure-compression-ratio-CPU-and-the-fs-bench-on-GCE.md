---
id: TASK-29.4
title: 'Measure compression: ratio, CPU, and the fs bench on GCE'
status: To Do
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
labels: []
dependencies:
  - TASK-29.3
parent_task_id: TASK-29
type: task
ordinal: 98000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Compression trades CPU at seal, upload and page-in for disk and bucket bytes and for transfer time. The zstd level and the cost on the guest critical paths (page-in latency, commit and seal) need measuring on GCE, where decisions about performance are made.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The bench (scripts/bench-gce.sh, all groups including pgbench) runs on GCE with compression, and its results are compared with the last uncompressed run in docs/measurements
- [ ] #2 Compression ratio and the bytes uploaded for the bench workloads are recorded
- [ ] #3 The zstd level is chosen from measurements, and the choice is documented in doc-7
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
