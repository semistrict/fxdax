---
id: TASK-395.5
title: >-
  fxdax-gc sweeps every cell with bounded concurrency, across any number of
  collector processes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 19:12'
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
- [x] #1 `Bucket` lists by delimiter (S3, GCS, Azure, file and memory stores, and the S3 test server), and the sweep finds cells by it, never listing their objects to find them
- [x] #2 One process runs up to N cell passes at once (a flag); two or more processes over one bucket split the cells, a cell's lease keeping two passes off one cell, and a process dying leaves its cells to the next sweep
- [x] #3 Moves footers are read, and objects compacted, concurrently within a pass; deletes use the store's batch delete where it has one (S3 DeleteObjects)
- [x] #4 A sweep reports, per pass and in total, cells visited, requests, bytes deleted and compacted, and the slowest cells, as text and as metrics
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

Round 2 (@claude, 2026-11-13), branch t3/395-sweep on t3/395-per-cell-collector: AC #1's second half, #2, #4, #5's code and harness.
1. Bucket::list_delimited(prefix, Page { after, max }) pages (Listing::next): S3/GCS/Azure through object_store's PaginatedListStore (continuation tokens), files/memory/sim cut pages from the whole level; the S3 test server pages (max-keys, continuation-token). cell::listed a stream: a page of tenants and of cells held at a time.
2. Collector: Holding { lease, since }; <C>gc/lease a layout::GcLease { epoch, holder, until, ended }: a pass takes the cell unless held unexpired (Error::Left(Left::Held)) or passed since its sweep began (Left::Swept); renews between steps; lets go at its end. Lease 0 (tests, sim): requests as before.
3. extent_remote::sweep::Sweep: passes at once (buffer_unordered over the cells' stream), each pass's requests counted by kind (Counted wrapper), Passed per pass, Swept totals with the slowest passes. Report gains deleted_bytes, compacted_bytes.
4. fxdax-gc: --at-once (16), --lease (600 s), --metrics FILE (Prometheus textfile, peak RSS); a line per pass that did something, two lines of totals.
5. fxdax-gc-fill (bucket generator), scripts/gc-sweep.sh (local), scripts/lib/gc-sweep-linux-host.sh + scripts/gc-sweep-host.sh (ubu1, fxjob --bench).
6. Tests: sweeps at once split the cells; a process gone's cells collected after their leases expire; passes at once bounded; the report matches a counted bucket; lease rules; paging (conformance, S3 server, cells); fxdax-gc output and metrics; the sim checks report bytes.
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

ubu1 run t395w1 on main 09863b785 (a1348601d with TASK-395.7): workspace 2563/2563 passed, sim 55/55, guest tests exit 0. Linux DoD item for this round's code met; the task's remaining ACs wait on the per-cell pass.

Round 2 (@claude, 2026-11-13), branch t3/395-sweep on t3/395-per-cell-collector (bb07304a6): AC #1's second half, #2, #4, and #5's code and harness.
Landed:
- Paged listings by delimiter: Bucket::list_delimited(prefix, Page { after, max }) gives a page (Listing::next says where the next starts). Store pages through object_store's PaginatedListStore for S3, GCS and Azure (continuation tokens, max_keys), kept beside the ObjectStore (store::reach, Store::paged); files, memory, the simulation and a degraded store (FXDAX_REMOTE_FAULTS) cut pages out of the whole level (Listing::page). bucket::list_delimited lists a level whole, page by page, for within-cell levels. Each page is now one counted request. The S3 test server pages (max-keys, continuation-token, NextContinuationToken), so non-delimited listings of over 1,000 objects page too.
- cell::listed is a stream: one page of tenants and one of a tenant's cells held at a time, each page fetched as the cells before it are taken.
- The cell's lease is layout::GcLease { epoch, holder, until, ended }. Collector::holding(Holding { lease, since }): a pass leaves its cell when another holds it unexpired (Error::Left(Left::Held)) or when another pass over it ended after its sweep began (Left::Swept), writing nothing. It renews the lease between steps (after reading the cell, before deleting) and lets it go at its end (ended set only when it went through). With lease 0 (the default: tests, the simulation, fxdax's in-process collector), the requests are as before. Collector::collect stays a plain loop, now over the stream, skipping Left.
- extent_remote::sweep::Sweep: runs `at_once` passes at once (buffer_unordered over the cells' stream, cells taken only as a slot frees). Each pass's requests are counted by kind (a Counted wrapper per pass, and one for the listing). observe() gets each Passed (cell, outcome, requests, took). Swept gives totals: visited, held, swept, failed, Report, marked bytes, requests, the slowest K and took. Report gained deleted_bytes and compacted_bytes (listed sizes).
- fxdax-gc: --at-once (16), --lease SECONDS (600), --metrics FILE (Prometheus text, written atomically after each sweep: cells by outcome, objects and bytes, requests by kind, a pass-time histogram, the slowest cells, peak RSS from getrusage). Text output: a line per pass that deleted, compacted, retired or failed, then a totals line and a timing/slowest line. A dry run stays serial, holding no lease.
- Measurement: bin fxdax-gc-fill (a file bucket of TENANTS x CELLS cells, each with a branch of SEGMENTS segments, half freed whole and half a quarter live, plus a largest cell; written into memory per cell, then to files without fsync, in parallel). scripts/gc-sweep.sh (fill once, then for each count in FXDAX_GC_AT_ONCE a hard-link copy, swept twice, with a table of seconds, deleted, compacted, requests and peak MiB). scripts/lib/gc-sweep-linux-host.sh and scripts/gc-sweep-host.sh (ubu1, fxjob --bench, results in docs/measurements/ubu1-gc-sweep-*). Defaults: 100 x 100 cells x 95 segments plus a largest of 950, so 1,000,855 objects in 10,000 cells.
Tests (new): sweep::tests::sweeps_at_once_split_the_cells (two sweeps over one SimBucket, requests completed in random order, 8 seeds: each cell passed once, by one of them; what one left the other ran); cells_of_a_process_gone_are_collected_once_their_leases_expire (a sweep dropped mid-delete holds 3 cells, which the next sweep leaves; after the lease expires the following sweep collects them); sweeps_run_so_many_passes_at_once (requests waiting from at most N cells at once, and exactly N reached, for N = 1 and 4); sweeps_report_what_the_bucket_counts (Store over memory: deleted and compacted counts and bytes equal to the bucket's before/after, every request kind equal to Store::stats, per-pass reports summing to the total, slowest sorted); passes_leave_cells_held_or_swept_since (lease rules: held, expired, swept, lease 0). Paging: conformance pages by 1, 2 and 3 (memory, file, sim, S3 server); s3_deletes_in_batches_and_lists_by_delimiter pages 250 objects in 3 requests; cell::cells_are_listed_a_page_at_a_time (2,005 cells of a tenant: exact requests, each page fetched when needed). fxdax-gc: flags, output lines, exact metrics text, tests/gc.rs end to end (--metrics file). The sim's check_report now checks deleted_bytes and compacted_bytes exactly. scripts/tests/test_host.py: the driver runs gc-sweep-linux-host.sh as a measurement.
Driver tested locally: FXDAX_GC_TENANTS=10 FXDAX_GC_CELLS=10 FXDAX_GC_SEGMENTS=95 FXDAX_GC_LARGEST=300 FXDAX_GC_AT_ONCE="1 4 16" scripts/gc-sweep.sh on macOS (10,205 objects in 100 cells, file bucket): first sweeps deleted 4,803 and compacted 4,802 in 30.6/23.1/21.6 s at 1/4/16 at once (bound by APFS fsync), peak 13/15/19 MiB; second sweeps 4.6/3.8/3.8 s.
To measure (AC #5), from a clean tree at the pushed commit: FXDAX_HOST_RUN=<run> FXDAX_HOST_COMMIT=<sha> scripts/gc-sweep-host.sh
Decisions in OPEN_QUESTIONS.md.
Bug found by the simulation (first scripts/test.sh run: 8 sim profiles failed with "collector: the cell is left to another pass of the collector, which holds it"). A pass holding its cell for no time (tests, the sim) read the clock before reading the lease and wrote until = that time. A racing lease-0 pass that took the lease in between looked unexpired, so the cell was left as held. Blind spot: the lease tests never raced two lease-0 passes while the clock moved, and the sweep tests use leases of a minute. Fix: lease-0 passes write until 0 (holding nothing), and every take reads the clock when it decides, after its read. The sim's profiles are the class test; they pass with the fix. Structure: fine; the clock read is now in the one place that decides.
Verified (macOS, commit a02dc9fa6): cargo check --workspace --tests clean; cargo clippy --workspace --tests (nothing new but pre-existing warnings); scripts/test.sh exit 0 (clippy gate, workspace 2334/2334, sim 59/59, Shuttle 37+10+6+33+10+18, scripts 313 OK, mutant checks 0 broken, go ok); cargo nextest -p extent_remote 278/278 after the last edits; the Linux check (zig cross, cargo check --workspace --tests --target x86_64-unknown-linux-gnu) clean. Not run: ubu1 (the parent's).
To measure (AC #5), from a clean tree: FXDAX_HOST_RUN=<run> FXDAX_HOST_COMMIT=<head of t3/395-sweep> scripts/gc-sweep-host.sh
<!-- SECTION:NOTES:END -->
