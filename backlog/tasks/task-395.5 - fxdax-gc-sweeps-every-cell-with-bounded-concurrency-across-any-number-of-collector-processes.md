---
id: TASK-395.5
title: >-
  fxdax-gc sweeps every cell with bounded concurrency, across any number of
  collector processes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 09:19'
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

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Scope of this round (agent @claude, 2026-11-13): AC #1's delimiter listing in Bucket and AC #3; AC #2 (the sweep over cells), #4 and #5 wait on the per-cell pass (TASK-395.1-.4).
1. Bucket::list_delimited(prefix) -> Listing { prefixes, objects }: one level of keys under a prefix, none below. Required of every bucket (no costlier fallback through a whole listing). Store over object_store's list_with_delimiter (S3, GCS, Azure, files, memory); SimBucket/Direct an op of their own; the test doubles forward it.
2. Bucket::delete_batch(keys): one request where the store batches (Store: object_store's delete_stream, S3 DeleteObjects of 1000, Azure's of 256), else one delete each, several at once (the default, which test doubles and the simulation keep, each delete a request of its own). bucket::delete_all: batches of DELETE_BATCH (1000), a few at once, each retried whole until it lands. Local (file buckets): its lock taken once a batch, not once an object.
3. Store counts both (Request::ListDelimited, Request::DeleteBatch; spans bucket.list_delimited, bucket.delete_batch; trace::PREFIXES attribute). S3 test server answers delimiter= with CommonPrefixes and counts its requests by kind.
4. Collector: moves footers read a hop at a time, every way's next moves at once (MovesMaps::read_listed, from the pass's listing, no listing of its own); compactions many at once (COMPACTIONS_AT_ONCE), all before what goes is worked out, as now; deletes through bucket::delete_all, after the fence check, as now (deletes are unordered already).
5. Tests: conformance gains delimiter listing and batch deletes (memory, file, simulated, S3 server; RustFS/GCS/Azurite through their ignored runs); faults wrapper fails both; S3 server request counts show a pass's deletes in DeleteObjects batches; served_in_rounds shows moves footers and compactions in one round.
6. clippy, nextest -p extent_remote, scripts/test.sh; commit; rebase; push to main.
<!-- SECTION:PLAN:END -->
