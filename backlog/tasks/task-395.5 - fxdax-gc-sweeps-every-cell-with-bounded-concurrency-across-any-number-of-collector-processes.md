---
id: TASK-395.5
title: >-
  fxdax-gc sweeps every cell with bounded concurrency, across any number of
  collector processes
status: To Do
assignee: []
created_date: '2026-11-13T08:53:00Z'
labels:
  - bucket
  - collector
dependencies:
  - TASK-395.4
parent_task_id: TASK-395
priority: high
ordinal: 777000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With a pass per cell, collecting a bucket is running passes over all its cells: millions of them. The sweep enumerates cells without listing their objects, runs passes concurrently, and lets several processes share the work, each cell's lease keeping two from colliding. Today's pass also does work one request at a time that bounds any sweep: moves footers read serially, compactions serially, deletes one object per request.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 `Bucket` lists by delimiter (S3, GCS, Azure, file and memory stores, and the S3 test server), and the sweep finds cells by it, never listing their objects to find them
- [ ] #2 One process runs up to N cell passes at once (a flag); two or more processes over one bucket split the cells, a cell's lease keeping two passes off one cell, and a process dying leaves its cells to the next sweep
- [ ] #3 Moves footers are read, and objects compacted, concurrently within a pass; deletes use the store's batch delete where it has one (S3 DeleteObjects)
- [ ] #4 A sweep reports, per pass and in total, cells visited, requests, bytes deleted and compacted, and the slowest cells, as text and as metrics
- [ ] #5 Measured on ubu1 against a file bucket or RustFS of at least 1M objects in 10k+ cells: a sweep's peak memory bounded by its concurrency times the largest cell, and its wall time recorded in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
