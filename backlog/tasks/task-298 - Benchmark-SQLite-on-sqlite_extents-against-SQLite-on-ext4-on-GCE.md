---
id: TASK-298
title: 'Benchmark: SQLite on sqlite_extents against SQLite on ext4, on GCE'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 09:55'
updated_date: '2026-11-09 10:04'
labels: []
dependencies: []
ordinal: 699000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user asked (2026-11-09) for a perf test of the SQLite VFS on an extent device (TASK-296) against stock SQLite, on GCE. A local macOS smoke run showed reads through the VFS ~100x slower than stock SQLite (each page read goes to the image with O_DIRECT, no cache below SQLite's own) and commits ~3 ms.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 crates/sqlite_extents/examples/sqlite-bench runs commit, bulk, update, batch, read and scan workloads on the VFS over an fxdax image and on stock SQLite (rollback FULL, WAL FULL, WAL NORMAL)
- [x] #2 A GCE run's results are in docs/measurements with a note
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the sqlite-bench example and ran it twice on GCE (c3-standard-8, pd-balanced): commits match rollback FULL (337/s vs 365/s), WAL FULL is 3x faster; reads are ~240x slower (987/s vs 236k/s) for want of a block cache: TASK-300. Results in docs/measurements/gce-sqlite-bench-2026-11-09T0955Z-abe0c4c0.
<!-- SECTION:FINAL_SUMMARY:END -->
