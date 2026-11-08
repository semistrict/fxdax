---
id: TASK-264
title: 'fxdax-gc: --dry-run and --mark-only, with readers warning of marked keys'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-08 19:49'
updated_date: '2026-11-08 20:31'
labels:
  - gc
  - extent_remote
dependencies: []
priority: medium
ordinal: 694000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Before trusting the collector to delete in a bucket that matters, an operator needs to see what it would delete (dry-run, writing nothing, not even gc/lease, which would fence a real pass), and to run it for a while marking instead of deleting while every reader warns if it reaches a marked key, and the collector warns if a key it found doomed is needed again: either is a bug in the collector, since no correct fork or recovery reaches what no live branch reached. Compacted originals are the most interesting keys to watch (TASK-175), so mark-only compacts as a real pass does.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 `fxdax-gc URL --dry-run` writes nothing to the bucket (gc/lease included), compacts nothing, and prints each doomed key with its kind and size, and totals, saying it undercounts what a real pass deletes
- [x] #2 Its doomed list equals what a real pass with --compact-below 0 deletes on the same bucket (test)
- [x] #3 `fxdax-gc URL --mark-only` compacts as a real pass does, deletes nothing, and writes gc/marks/<epoch> holding exactly the doomed keys with size, version/etag and modified time (test)
- [x] #4 The lease decision for mark-only is reasoned from doc-7 fencing (step 1, TASK-180) and recorded in the code and here
- [x] #5 Every reader of a bucket (extentd, host stores, writers recovering, fxdaxctl, the collector) warns when it GETs or HEADs a key the newest marks object names, with the key, its kind, the operation, request ID and span; one set lookup per read, nothing when no marks exist, refreshed on an interval, never a round trip per read (test)
- [x] #6 A mark-only pass warns of each key the previous marks named that is needed again, listed at the same version (test: a rule mutation dooms a live extent)
- [x] #7 Marks objects are never collected nor taken for branches objects (test)
- [x] #8 Usage text, module docs, parse tests and doc-7 Garbage collection describe both flags and say mark-only grows storage
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. layout: gc/marks/<epoch> key, Kind::Marks, the Marks format; never doomed.
2. collector: one survey shared by three passes: collect (delete), mark (takes the lease, compacts, writes marks unless fenced, compares with the previous marks for resurrections), dry_run (observes the lease epoch, writes nothing: no abandon/claim writes, no compaction).
3. marks.rs: a Watch embedded in store::Store (the concrete bucket every reader uses), refreshed every 60 s joined to a read, uncounted; warns on GET/GET range/HEAD of a marked key with kind, op, marks epoch, OTel trace/span IDs (request_id via the daemon logger); listings at debug.
4. fxdax-gc: --dry-run (no probe), --mark-only, stderr logger; fxdaxctl logger on Linux.
5. Tests: collector unit tests (dry-run unchanged + equals real pass, mark-only, resurrection via extent_keys_name_segments, marks never collected), store reader-warning test with captured log, fxdax-gc parse + CLI tests.
6. doc-7 update.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Lease for --mark-only: taken, as a deleting pass takes it (doc-7 GC step 1). Marks keyed by the epoch are totally ordered and unique, so readers and the next mark-only pass find the newest by key; a deleting pass it fences is late, never wrong; and TASK-180 stays decidable: a mark-only pass finding a live branch's object lost can tell a deleting pass that fenced it (stop quietly) from a bug. Fenced, it writes no marks, as a fenced pass deletes nothing. --dry-run instead reads gc/lease's epoch at its start (no write) and reports fenced if it changed by the end; it also skips the fxdax-gc probe (which writes probe/<nonce>) and the abandon/abandon_claim writes, taking abandonable forks of deleted points as given up on.

Reader warnings live in store::Store (marks::Watch), not a generic wrapper type: every reader (extentd, host_store, remote, writers, fxdaxctl, the collector) holds a concrete extent_remote::store::Store, so embedding makes it on everywhere with no type changes. Refresh: every 60 s (marks::REFRESH), a LIST of gc/marks/ joined to a read (plus a GET only when the newest changed); its requests go uncounted in Stats (tests assert exact request counts). Hot path: one relaxed load with no marks, one HashSet lookup with marks. GET, ranged GET, HEAD warn with key, kind (branch), request, marks epoch, OTel trace/span IDs (trace::current_ids); in extentd the logger adds request_id/store. LIST entries log at debug only: listings routinely name what callers pass over and the protocol tolerates being gone (older checkpoints recovery lists past, segments a checkpoint covers whose seq a writer fences after, a fork parent's newer checkpoints), so warning on them would fire on every recovery under mark-only; what a caller goes by it reads, which warns. The collector's census (prefix "") is skipped entirely.

Resurrection: a mark-only pass compares with the newest marks before its epoch; a key marked then, listed now with same key/etag/version/size, not doomed, warns (RESURRECTED) and is returned in Marking::resurrected. Marks record their grace; marks of a shorter grace than the pass's are not compared (longer grace delays dooming compacted originals).

--dry-run accepts --compact-below and lists what a pass would compact (by listed size) apart from what it would delete. fxdax-gc gets a warn-level stderr logger; fxdaxctl installs fxdax::extentd::logging at warn on Linux.

Validation: cargo nextest run -p extent_remote -p fxdax 573/573 (macOS); scripts/test.sh clippy part clean; Linux cargo check -p fxdax -p extent_remote --tests --bins clean in rust:1 Docker.

DoD: workspace suites green on Linux and macOS in main CI 42519109512 (test (workspace), macos (workspace)), not on GCE; scripts/upstream-changes.py --check passes; no upstream code changed (extent_remote and fxdax are fxdax-own).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax-gc --dry-run (writes nothing, gc/lease included; lists delete/compact candidates with totals, says it undercounts) and --mark-only (takes the lease, compacts, writes gc/marks/<epoch>, warns RESURRECTED of objects the previous marks named that are kept at the same version). store::Store warns on GET/ranged GET/HEAD of marked keys (refresh every 60 s, uncounted, one load or one set lookup per read). Verified: collector tests (dry run leaves every object byte for byte and equals a compact_below 0 pass; mark-only marks exactly what a pass deletes, deletes nothing; a wrong pass via extent_keys_name_segments=false makes a fresh reader warn on both reads of the extent and the next mark-only pass warn RESURRECTED; marks survive a deleting pass), a store test, fxdax-gc parse and CLI tests; macOS extent_remote+fxdax 573/573; main CI 42519109512 green at 056e36f8a.
<!-- SECTION:FINAL_SUMMARY:END -->
