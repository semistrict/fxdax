---
id: TASK-88
title: >-
  extent_remote's simulation builds segments with Batch, not with extent_store,
  so the store's footer contents never meet takeovers or collectors
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-22 13:26'
updated_date: '2026-11-09 10:21'
labels:
  - review-2026-10-21
dependencies:
  - TASK-165.9
priority: medium
type: task
ordinal: 262000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Left from the object-storage protocol refactor (integrate-2, review finding 'Segment footers come from two producers'). At dde8d58d3 the producers are still two: the simulation's sessions close segments with batch::Batch (crates/extent_remote/src/batch.rs:126-166, Batch::contents and close_forking; crates/extent_remote/src/sim/world.rs:11, 1090), and a store closes its own (Segments::close, crates/extent_store/src/segments.rs:803-841, from its Window, segments.rs:1214-1299). Since TASK-165.3 both make one shape of footer, a chain place and Contents (a8ca6cb26), under one spending rule (530248363), applied by one replay (f3f6b616f), so they can differ only in what a segment's Contents say. But extent_store's choice of what a window seals, frees and forks, and its next_sequence at a close, is still never run against takeovers or the collector. The review traced a next_sequence/orphan case (TASK-79.3) that the simulation therefore can't reach.

Each session would need an extent_store Store of its own, attached as fxdax's remote.rs does (start, crates/fxdax/src/remote.rs:1132-1222, diffing a snapshot). extent_store has depended on extent_remote since f3f6b616f, so the simulation can't build a Store where it is, in extent_remote, without a cycle: it moves to extent_store, or to a crate above both. TASK-165.9 moves the uploader into extent_store and deletes the attach's diff, which is where to do it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The simulation's writers commit through extent_store's Store, and their segments' footers come from it
- [x] #2 The simulation reaches TASK-79.3's case: a test seeds it and fails before TASK-79.3's fix
- [x] #3 The extent_remote simulation tests pass, in a time comparable to now
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. extent_remote: a `testing` feature (on in its own tests) that makes the protocol rules switchable (Rules, set_rules; cfg(any(test, feature))) and exposes what the sim needs that is private now: SimBucket/Direct/Fault (sim/bucket.rs moves to extent_remote::testing), moves::{MovesMaps, Moved}.
2. extent_store upload.rs: the uploader generic over the bucket (Shared<D, B: Bucket>, Writer<B>), its steps (start, gather, acknowledged, after/retake) callable without its tokio loop: probe moved to open(); blocking() runs inline outside a multi-thread runtime. Production keeps Remote<D> over extent_remote::store::Store (default type parameter).
3. The simulation moves to extent_store/src/sim (cfg(test)), extent_store dev-depending on extent_remote/testing. A session is a Store over its host disk (MemDir::serial, deterministic I/O) and an uploader Shared over SimBucket whose steps the world schedules as tasks/events, as now. Guest actions: write extents (open, uncommitted), commit (seals, frees, root, capacity), close (gather with close: close_now between commits), fork (commit_fork). Batch and Wal<Data> leave the sim. Host crash: MemDir::crash(seed); disk lost: dropped. Commit identity: the sim maps (branch, footer tag) to its own commit tags, registering each store epoch as committed and each tag as gathered.
4. Checks as now, plus: the store names what it reads where the store itself says (its places), replacing the names mock (TASK-195).
5. scripts/test.sh sim part, mutants.sh (extent_remote mutants killed by extent_store sim tests), doc-7/doc-14/doc-15 references updated.
6. AC #2: a mutant putting back TASK-79.3 (collector deletes an own extent object by next_sequence) killed by a seed with writes between commits and close_now. AC #3: time the sim part before/after.
Verify: cargo nextest of extent_store sim tests (profile sim), mutants.sh, scripts/test.sh parts touched.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: description brought to dde8d58d3: TASK-165.3 made one footer shape and one replay, leaving two producers of Contents; extent_store now depends on extent_remote, so the simulation can't build a Store from extent_remote.

2026-11-06 tidy: depends on TASK-165.9, which moves the uploader into extent_store and deletes the attach's diff: where its description says to do this.

Branch task-88 at 12733b212 (worktree ~/.t3/worktrees/fxdax/task-88). The sim runs real Stores (MemDir::serial) and the real uploader loop (upload::Shared, generic over the bucket, its interval on a Clock). AC1 done. AC2: sim::collection_needs_extent_keys_naming_segments (rule extent_keys_name_segments off = TASK-79.3 pre-fix) finds the case at seed 115 of everything; the old sim none in 4,000/4,000/20,000 seeds of everything/collected/takeovers. AC3: sim part 46/47 pass in 70 s (macOS, profile sim); everything 4.6 s vs 2.7 s before. Open: late_checkpoints_need_ways_from_seals finds no seed in 40,000 of recreated or late_checkpoints (TASK-175 case: a fenced writer that never wrote checkpointing late). Found and filed: TASK-295 (fixed on the branch), TASK-293 (open; check_compacted mirrors the prefilter meanwhile).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The simulation of doc-7's protocol now runs on extent_store's real stores (MemDir::serial, deterministic I/O) and the real uploader loop (upload::Shared, made generic over the bucket and run on a Clock the simulation alone runs out), so the store's seals, frees, forks, footers, WAL and rebasing meet takeovers, crashes and collectors. It moved from extent_remote to extent_store, extent_remote exposing its SimBucket, moves and rule switches behind a `testing` feature. Verified: sim part 46/46 in 38 s (profile sim, macOS, on origin/main fcf318cfe), extent_store+extent_remote suites 370/370, both hand mutants killed, scripts tests 257 OK, Linux compile in a rust:1 container. AC2: with TASK-79.3's pre-fix rule (extent_keys_name_segments off) the new sim loses an own extent at seed 115 of everything; the old sim found none in 28,000 seeds. Found and filed: TASK-295 (dropped WAL resurrected by a crash; fixed with a store test), TASK-293 (collector never compacts compressed mostly-dead segments; open). Per the user, the ways_from_seals sim search was removed (reached at no seed of 40,000; the old sim only at seed 677), its rule shown by the collector unit test and TLA+.
<!-- SECTION:FINAL_SUMMARY:END -->
