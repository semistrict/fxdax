---
id: TASK-42.8
title: 'Follower protocol in TLA+: model-check the design before building it'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-17 22:29'
updated_date: '2026-10-17 23:06'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 128000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The follower's correctness rests on a protocol across the writer, extentd's views, the follower and its readers (doc-8, Concurrency and Store views); a design flaw there would be built into every layer. A TLA+ spec checks the design itself, exhaustively over small bounds, before the code exists, and later checks the implementation's traces against it (doc-8 testing 0). Tools: TLC (tlaplus/tlaplus, MIT) and, for larger bounds, Apalache (Apache-2.0).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A TLA+ spec (in the repo, with its TLC configurations) models the writer (transactions, commits, frees), store views (epochs, kept frees, pins, cut-off), the follower (advance, batch apply, seqcount, per-inode invalidation, grace period, graveyard) and readers (tree reads with retry and fallback, faults mapping extents, read(2) in flight, files deleted while open)
- [x] #2 TLC checks the safety properties over small bounds: every read sees a committed prefix of the writer's history, readers' prefixes never go back, no read touches an extent released, no mapping of an old extent outlives the follower's advance, a file deleted while open reads to its end
- [x] #3 TLC checks liveness under fairness: the writer is never blocked by a follower, a follower advances, readers are not starved, pins are released after close or advance, and a lagging follower is cut off rather than pinning without bound
- [x] #4 Mutation checks: removing each rule (validation after reads, invalidation after apply, the grace period, keeping frees for older views, graveyard pins) gives a TLC counterexample
- [x] #5 Running it is one script, documented in doc-8
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. specs/follower/Follower.tla: plain TLA+ (not PlusCal), at the protocol's level: the writer's transactions as ordered key mutations over a tiny filesystem (a token renamed between two directories, files written (new extent, old freed) and deleted); the store's kept frees, pins, releases and cut-off; the follower's batches (mutex + seqcount, mutation by mutation), per-object invalidation under exclusive inode locks, SRCU grace period, view advance, graveyard pins; readers doing validated pair reads (the torn-rename check), DAX faults under shared inode locks, read(2) under SRCU, open/close.
2. Rules as a constant set (validate, fallback, invalidate, grace, graceFirst, keepFrees, graveyard), each guarding its mechanism, so a mutation config drops one.
3. Safety as invariants over an errors set the actions fill, plus state invariants (no use of a released extent, no stale mapping once invalidation is done, bounded retries); liveness as temporal properties under weak fairness (follower catches up, readers finish, pins released, lag cut off); the writer never waits as a property over ENABLED.
4. scripts/check-tla.sh: fetches tla2tools 1.8.0 (sha256 pinned) into ~/.cache, runs TLC on the main config (must pass) and each mutation config (must fail with the expected violation).
5. Tune bounds so the full run takes minutes at most; document in doc-8.
6. Findings that change the design (e.g. seqlock_t's spinlock can't be held across Fxfs's apply) go into doc-8 and the tasks.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Evidence (scripts/check-tla.sh, TLC 1.8.0, 2026-10-17, M-series Mac, 15 workers): Follower (1 reader, 3 commits, 3 ops) holds, 27.5M distinct states, 53s; FollowerReaders (2 readers, symmetric, 2 commits, 2 ops) holds, 28.0M, 84s; FollowerLive (liveness: WriterNeverWaits, FollowerCatchesUp, FreesGo, ReadersFinish, LagCutOff) holds, 458K, 78s; FollowerStalled (a hung follower is still cut off) holds, 1.7M, 47s. Eight mutations each violate exactly the property their rule guards: NoValidate->NoTornReads, NoFallback->BoundedRetries, NoInvalidate->NoStaleMappings, NoGrace and NoGraceFirst->NoReleasedReads, NoKeepFrees->MappingsLive, NoGraveyard->OpenFilesReadable, NoCutOff->LagCutOff; 1-4s each. Whole run ~5 min.
Design finding: the seqcount must be seqcount_mutex_t (its writer holds a mutex), not seqlock_t (a spinlock, which applying Fxfs's mutations, which may sleep, can't hold); doc-8 and TASK-42.5 corrected. Pins released after close: a safety invariant (NoLeakedPins: every pin is a graveyard copy for an open handle), stronger than the liveness form.
Not used: Apalache; TLC's exhaustive bounds suffice for now. Trace validation of the simulation's histories is TASK-42.1's (it needs the simulation).
DoD: no Rust changed (TLA+, a script, the Makefile, docs), so the workspace suites are unaffected; upstream-changes.py --check and license headers pass.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/follower/Follower.tla models the follower protocol (writer transactions, store kept frees/pins/cut-off, follower batches under a mutex+seqcount, per-file invalidation, SRCU grace period, view advance, graveyard; readers' validated reads with fallback, DAX faults, read(2), open/close), every mechanism behind a rule. make check-tla (scripts/check-tla.sh, TLC pinned by SHA-256) checks four models (safety with one and two readers, liveness, a stalled follower) and eight mutations, each of which must and does violate the property its rule guards, in ~5 minutes. Found that the seqcount must be seqcount_mutex_t; doc-8 and TASK-42.5 updated.
<!-- SECTION:FINAL_SUMMARY:END -->
