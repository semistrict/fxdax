---
id: TASK-29.4
title: 'Measure compression: ratio, CPU, and the fs bench on GCE'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 13:35'
updated_date: '2026-10-19 05:42'
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
- [x] #1 The bench (scripts/bench-gce.sh, all groups including pgbench) runs on GCE with compression, and its results are compared with the last uncompressed run in docs/measurements
- [x] #2 Compression ratio and the bytes uploaded for the bench workloads are recorded
- [x] #3 The zstd level is chosen from measurements, and the choice is documented in doc-7
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured 2026-10-19 on GCE c3-standard-8, Fxfs in the guest over DAX (four request queues), branches in GCS (gs://echophase-fxdax-bench/zstd), levels on one host one after another (FXDAX_ZSTD_LEVEL; 0 stores parts raw): docs/measurements/gce-fs-bench-zstd-{0,1,3} (all groups but pgbench), gce-fs-bench-pgbench-zstd-{0,1}.
- pgbench, level 0 -> 1: init 5.00 s -> 4.10 s; 1,262 -> 1,442 tps (+14%); uploaded 1.9 GiB -> 123.6 MiB (15x).
- Other workloads, 0 -> 1 -> 3: uploaded 1.2 GiB -> 704.7 MiB -> 705.5 MiB; seq-write 70.3 -> 65.8 -> 61.8 MiB/s; the rest within noise.
Chosen: level 1 (doc-7, decision 7). An earlier attempt of 10-18/19 ran levels 1 and 3 on a tree mid-change (the extent device's queues), which hung the guest: rerun on the committed tree.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
zstd level 1 chosen from GCE measurements: against raw parts it uploads 15x less for pgbench and runs it 14% faster; level 3 compresses no further and writes slower. Recorded in doc-7 and docs/measurements.
<!-- SECTION:FINAL_SUMMARY:END -->
