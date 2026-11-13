---
id: TASK-395.8
title: >-
  Usage per tenant and cell from the collector's passes, not a listing of the
  whole bucket
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13T08:53:00Z'
updated_date: '2026-11-13 23:00'
labels:
  - bucket
  - collector
  - tools
dependencies:
  - TASK-395.4
  - TASK-395.6
parent_task_id: TASK-395
priority: low
ordinal: 780000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-394's `fxdax-gc --usage` lists the whole bucket into memory: 30-40 GB and hours at 100M objects. Each cell pass lists the cell anyway, so it can record the cell's objects and bytes, and usage becomes reading those records.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Each cell pass records its cell's objects and bytes (by kind) under the cell, with the time of its listing
- [x] #2 `fxdax-gc --usage` reads the recorded usage of every cell (or of one tenant's), streaming one JSON object a line per tenant and per cell, in memory bounded independently of the bucket's size, each with when it was measured
- [x] #3 A test shows usage after writes, compaction and deletion matching a listing of the cells
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Each full pass records Usage {measured (listing's start, store clock), kinds: objects/bytes by layout kind name} in <C>gc/lease: its listing less what it deleted, plus the moves it compacted to; the lease counts itself as written.
2. fxdax-gc --usage [--tenant TENANT]: tenants by delimiter listing, a tenant's cells by delimiter listing, each cell's lease GET (32 at once, in order): a JSON line per cell then per tenant (IDs hex, measured_unix_ms, objects, bytes, kinds); nulls for a cell no pass ended on. Memory: one tenant's cell IDs. TASK-394's whole-bucket listing removed.
3. Tests: line formats (unit), collector test that recorded usage matches a listing after writes, compaction and deletion, CLI test over a file bucket (usage vs listing, measured time window, unpassed cell nulls, --tenant).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round 1 (@claude, 2026-11-13), batch with TASK-395.6 on branch t3/395-incremental-usage.
Landed: each full pass records Usage {measured: its listing's start by the store's clock, kinds: objects/bytes by layout kind name} in <C>gc/lease: the listing less what it deleted, plus the moves it compacted to; the lease counts itself (bincode fixint keeps its length). Skipping passes don't re-measure. fxdax-gc --usage [--tenant TENANT]: cell::tenants and cell::of_tenant by delimiter listings, each cell's lease read 32 at once in order, a JSON line per cell ({scope, tenant, cell, measured_unix_ms, objects, bytes, kinds}; nulls for a cell no pass ended on) then per tenant (cells, measured_cells, oldest/newest measured, sums). IDs hex. TASK-394's whole-bucket listing removed (its unowned/total lines with it).
Tests: collector::tests::passes_record_what_their_cells_take (usage == listing by kind after writes + compaction + deleting the compacted, and after a deletion with the cell reclaimed and the reference retired; unchanged cells keep their older measurement); tests/gc.rs usage_says_what_each_files_cell_and_tenant_takes_as_passes_found (file bucket via the binary: per-cell lines equal a listing, measured within the pass's window, unpassed cell nulls, tenant sums, --tenant, writes nothing); fxdax-gc unit tests for the line formats and flags.
Verified as in TASK-395.6's notes.
ACs: #1 and #3 proven. #2 left unchecked: streams JSON per tenant and cell with times (tested), but memory holds one tenant's cell IDs at a time, since Bucket::list_delimited returns a level whole: bounded by the largest tenant's cell count, not independent of the bucket's size. A paged delimiter listing (start-after) would close it.

Integration (@claude, 2026-11-13, branch t3/395-integrated): with TASK-395.5's paged listings, cell::tenants, cell::of_tenant and cell::listed are streams of bucket::prefixes (a page of 1,000 at a time, each listed once those before are taken); fxdax-gc --usage goes over them, holding a page of tenants, a page of cells and 32 lease reads at a time. Test: fxdax-gc tests::usage_lists_a_page_of_cells_at_a_time (Store over memory, 2,005 cells of one tenant among others: every cell line once in order; at each line of the big tenant's, the delimiter pages listed so far are exactly those its next 31 lease reads need; 6 pages in all). tests/gc.rs usage test updated to the sweep's output.

Verified on t3/395-integrated (macOS): cargo clippy --workspace --tests (nothing new); cargo nextest -p extent_remote 296/296 (with the 2 new tests); scripts/test.sh: clippy gate, workspace 2352/2352, sim 61/61 (deletions_need_fences re-seeded, see TASK-403), Shuttle 37+10+6+33+10+18, scripts 313 OK, mutants 0 broken, go ok; Linux zig cross-check clean; TLA+ specs/bucket models but BucketForks/Overlap/Strays/Takeovers hold (16) and all 72 mutations fail as expected. Flake TASK-404 filed (commits_reopen_from_their_logs under load, unrelated). Not run: ubu1.

AC #2 checked: tests::usage_lists_a_page_of_cells_at_a_time.

ubu1 runs t395w4 (ba19ced7f: workspace 2623/2623, sim 61/61, every other nextest part, mutants 0 broken) and t395w5 (22602dbd1, the lock fixed: Firecracker 30/30 and 31/31, scripts 313 OK, guest tests pass); macOS scripts/test.sh passed on the integration (workspace 2352/2352, sim 61/61); scripts/upstream-changes.py --check passes. On main as 22602dbd1.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Each full cell pass records its cell's usage by kind with the listing's time in <C>gc/lease; fxdax-gc --usage [--tenant] streams one JSON line per cell and per tenant from those records, a page of tenants, a page of cells and 32 lease reads at a time; TASK-394's whole-bucket listing is gone. ubu1 runs t395w4 (ba19ced7f: workspace 2623/2623, sim 61/61, every other nextest part, mutants 0 broken) and t395w5 (22602dbd1, the lock fixed: Firecracker 30/30 and 31/31, scripts 313 OK, guest tests pass); macOS scripts/test.sh passed on the integration (workspace 2352/2352, sim 61/61); scripts/upstream-changes.py --check passes. On main as 22602dbd1.
<!-- SECTION:FINAL_SUMMARY:END -->
