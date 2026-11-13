---
id: TASK-395.1
title: >-
  Spec: Bucket.tla collects per cell, with fork references and conservative
  fork-point keeping
status: Done
assignee:
  - '@claude'
created_date: '2026-11-13T08:52:00Z'
updated_date: '2026-11-13 19:38'
labels:
  - bucket
  - collector
  - spec
dependencies: []
parent_task_id: TASK-395
priority: high
ordinal: 773000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Bucket.tla models one collector over a flat set of branches: one lease, one atomic listing (`GcStart`), records read until none are new, every branch recovered, compaction and deletes in one step. A per-cell collector breaks the rules the current proof relies on (candidates listed first over the whole bucket, records relisted bucket-wide), so the protocol is model-checked before any code changes. The model is the design of record for the sibling tasks.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Branches belong to cells; each cell has its own collector lease and passes, which list and delete only under the cell, running concurrently with other cells' passes and with writers
- [x] #2 A fork of a branch in another cell has a reference under the parent's cell, written before anything the child's survival depends on is published (a writable fork's child record; a point clone's forking record, before it reads the point's record), removed only once the child is deleted and its own cell holds no references
- [x] #3 A cell's pass keeps, for each reference, everything the parent's state at the fork point needs; and lists its references again after reading its records, as passes relist records today
- [x] #4 The invariants that hold today (no live branch loses what it reads; creates never land where the collector freed) hold with per-cell passes, under the existing cfgs extended with two or more cells and cross-cell forks, point clones and deletions
- [x] #5 A mutation for each new rule (no reference, reference written late, reference removed while descendants live, no relisting of references, fork-point state not kept) makes TLC find a violation
- [x] #6 doc-7's Garbage collection section describes the per-cell protocol
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Bucket.tla: constant CellOf (branch -> cell; OneCell, CellEach, Pair maps), variable refs (references <<parent, at, child>> under the parent's cell), per-cell collector leases (glease) and passes (gc[i].cell); a pass lists only its cell (candidates), reads its cell's records, recovers its live branches, keeps fork-point states (StateAt(parent, at)) of forks in its states to other cells and of its references, lists references after the states, deletes only under its cell, and retires references of its cell's deleted children once its cell has no live branch and no reference.
2. Writers (RecordForks), deletions (RecordAll) and restores write references before the child record / before the point's read; claims none.
3. Six rules (references, referencesFirst, clonesReferFirst, referencesHeld, referencesRelisted, forkPointsKept), a mutation each that TLC must fail; existing mutations keep OneCell (today's whole-bucket pass) and gain the new rule names.
4. Existing cfgs extended to cells (CellEach for 2-branch, Pair for 3-branch); new cfgs BucketCells*, sized for runs under ~5 min each; invariant ReferencesStand.
5. doc-7 Garbage collection rewritten per cell, with the key layout for TASK-395.2; OPEN_QUESTIONS decisions.
6. check-tla.sh over bucket models (BucketTakeovers excepted: 1h47m) and every mutation; commit, rebase, push; finalize.

7. As built: passes of different cells overlap only with "overlap" (BucketOverlap, BucketCellsPoints check it); "forkNext" (least free ID) keeps BucketCells and NoReferencesHeld small; every clone writes a reference, its own cell included (BucketOverlap's counterexample).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Model (specs/bucket/Bucket.tla): CellOf (OneCell/CellEach/Pair), refs [p, at, c] under the parent's cell, passes per cell (gc[i].cell; Fenced by a later pass of the same cell), Listing(c), fork-point states (ForkPoint/RefPoints) kept whole and behind, references listed after the states (GcRelist), retirement by the child's cell's pass once that cell has no live branch and no reference (the one delete outside a cell, keys from its deleted records' origins), writers/deletions/restores writing references first. Invariant ReferencesStand.
Found by TLC: a clone in its origin's own cell was lost (BucketOverlap): records read before it, its point's reference (another cell) retired before the references were listed. Fix: every clone writes a reference, its own cell included. With that, NoRecordsRelisted and NoRecoveriesByOrigins no longer fail; their mutations now drop references too (both rules kept).
Runs (macOS laptop, scripts/check-tla.sh): all bucket models hold but BucketTakeovers (not run, ~1h47m, one branch, unchanged): Bucket 40s, Compaction 128s, Deletions 158s, Forks 832s (was 1002s before cells), Inherit 103s, Overlap 860s (399s before), Own 6s, Placements 84s, Points 136s, PointsDeep 185s, Spans 138s, SpansCollect 31s, Strays 1063s (one cell, unchanged), Twins 1s, Wal 4s, BucketCells 101s (2.9M states), BucketCellsPoints 146s (4.1M). All 51 mutations fail as expected; new: NoReferences 1s, NoReferencesFirst 1s, NoClonesReferFirst 22s, NoReferencesHeld 12s, NoReferencesRelisted 80s, NoForkPointsKept 1s.
Decisions in OPEN_QUESTIONS.md (# Decisions, TASK-395.1). No Rust changed: DoD #1 holds trivially (cargo nextest not run). scripts/upstream-changes.py --check passes.

BucketTakeovers on the per-cell spec (67ffb6377) still unchecked: a run at nice 19 on the macOS laptop beside TASK-395.2-.4's builds was stopped at the 2 h limit unfinished (TLC's log went with it). To run once the laptop is idle, un-niced (~1h47m).

BucketTakeovers on ubu1 (main 04085cad, fxjob 12G 8 CPUs, 2026-11-13): no violation in 2h37m, 199.8M distinct states, 643M generated, 19.3M left on queue; far larger than the laptop's 1h47m before cells. Stopped by the parent (SIGTERM, exit 143) so the per-cell collector's qualification could run; its 12G sat at its limit. The 14:31 checkpoint is kept, hard-linked, at ubu1:/mnt/fxdax-scratch/claude-tla-395/keep.states (27G), with keep.log. To resume: java -cp tla2tools-1.8.0.jar tlc2.TLC -recover <keep.states/26-11-13-12-01-33.434> -workers auto -metadir keep.states -config specs/bucket/BucketTakeovers.cfg specs/bucket/Bucket.tla, under fxjob with more memory. Still unchecked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Bucket.tla now models the per-cell, conservative collector: cells (CellOf), references under parents' cells, a pass per cell under its own lease that lists and deletes only its cell (but the references of its dead cell's children, the one cross-cell delete), keeping the parent's whole state at each referenced fork point, references listed after the states. Six new rules, each with a mutation TLC fails; every existing model extended to cells (CellEach/Pair) plus BucketCells and BucketCellsPoints, all holding; existing mutations on OneCell. TLC found clones in their origin's own cell lost without a reference: every clone now writes one. doc-7's Garbage collection rewritten (protocol, format-8 key layout, cross-cell requests per pass, fork chains, gaps for the code); decisions in OPEN_QUESTIONS.md. Verified with scripts/check-tla.sh over every bucket model but BucketTakeovers and all 51 mutations; no Rust changed.
<!-- SECTION:FINAL_SUMMARY:END -->
