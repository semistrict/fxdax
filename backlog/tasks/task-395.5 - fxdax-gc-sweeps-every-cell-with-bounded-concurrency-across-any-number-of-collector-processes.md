---
id: TASK-395.5
title: >-
  fxdax-gc sweeps every cell with bounded concurrency, across any number of
  collector processes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 10:07'
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
- [x] #3 Moves footers are read, and objects compacted, concurrently within a pass; deletes use the store's batch delete where it has one (S3 DeleteObjects)
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

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), commit a1348601d on main: AC #1's delimiter listing in Bucket and AC #3. AC #2 (the sweep over cells, which AC #1's second half also needs), #4 and #5 are left for after the per-cell pass (TASK-395.1-.4).
Landed:
- Bucket::list_delimited, required of every bucket with no fallback through a whole listing. Store uses object_store's list_with_delimiter (S3, GCS, Azure, files, memory); SimBucket/Direct have an op of their own; the S3 test server answers delimiter=/ with CommonPrefixes and counts its requests by kind; the Faulty wrapper fails it like any request.
- Bucket::delete_batch and bucket::delete_all: batches of 1,000, 4 at once, each retried whole. Store goes through delete_stream: one DeleteObjects per batch on S3, Azure's bulk requests, single deletes 10 at once on GCS and files. The trait's default (one delete per object, 32 at once) is kept by the simulation and test doubles, so their deletes stay single requests. File buckets take their lock once per batch.
- Store counts list-delimited and delete-batch requests (spans bucket.list_delimited and bucket.delete_batch; trace::PREFIXES); docs/tracing.md updated.
- Collector: moves footers are read a hop at a time, every way's next hop at once (MovesMaps::read_listed, from the pass's own listing); compactions run 8 at once (COMPACTIONS_AT_ONCE); deletes go through delete_all. Ordering kept: all compactions end before the doomed set is worked out (it comes from the listing taken before, which holds none of their moves), and deletes start only after the fence check. Deletes were already unordered.
Bug found: file buckets kept the directories of deleted objects forever, one per deleted branch, and a delimiter listing reported them as prefixes of nothing. Found by the new conformance check. Blind spot: conformance only listed whole prefixes, which skip directories. Fixed with LocalFileSystem's automatic cleanup; creates racing it are safe (a staged upload recreates its directory, and a non-empty directory is never removed). Structure: fine, a one-line setting.
Tests: conformance (memory, file, simulated, S3 server; RustFS/GCS/Azurite through their ignored runs) lists by delimiter and deletes a batch; faults::requests_fail_as_their_faults_say; store::s3_deletes_in_batches_and_lists_by_delimiter (2,500 deletes in 3 DeleteObjects, 0 DELETE, 1 delimiter listing); collector::passes_delete_in_batches_over_s3 (a pass's 5 deletes in 1 DeleteObjects); collector::passes_compact_and_read_moves_at_once (5 compactions, 5 moves footers and 5 deletes, each in one round).
Verified: cargo clippy -p extent_remote -p extent_store --all-targets clean for these changes (3 pre-existing warnings elsewhere); cargo nextest run -p extent_remote 240 passed, 9 skipped; scripts/test.sh passed (workspace 2290/2290, sim 55, shuttle 37 and the rest, scripts 307 OK, mutant checks, go). Two fxdax remote tests that listed every request kind were updated. No ubu1 run (the parent batches one). Decisions in OPEN_QUESTIONS.md.
<!-- SECTION:NOTES:END -->
