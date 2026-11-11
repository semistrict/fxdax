---
id: TASK-149.45
title: 'specs/bucket: model commits spanning segments'
status: Done
assignee:
  - '@claude'
created_date: '2026-11-10 16:32'
updated_date: '2026-11-11 08:20'
labels:
  - testing
  - tla
dependencies: []
parent_task_id: TASK-149
priority: low
ordinal: 734000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-92.41 made a commit span segments: a footer names the runs of the commit it goes on with (Contents::continued), readers hold them until a segment ends the commit and drop them at a fence, writers acknowledge only segments ending their commits, and Wal::rebase lands only between commits. specs/bucket/Bucket.tla models one commit per segment, so TLC checks none of those rules; extent_store's simulation and crash rounds, and extent_remote's tests, exercise them (commits_spanning_segments counted there).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Bucket.tla's segments may go on with a commit a later segment ends; readers seal its runs only with the end and drop them at a fence; writers acknowledge only segments ending their commits; the WAL lands only between commits
- [x] #2 A mutation for each rule (acknowledging mid-commit, landing mid-commit, readers applying a continuation alone) fails TLC; scripts/check-tla.sh runs them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. specs/bucket: segments carry a continuation (cont: the commit and a run of it); a commit may span: continuation segments, then the one ending it (Can span). Readers hold the runs (State.begun) until the ending segment, drop them at a segment neither going on with the commit nor ending it. Writers acknowledge only through segments ending commits. Wal::rebase lands only between commits (heads per prefix, as the code). A crash keeping the disk keeps the WAL to its last durable commit (Can cut), whole commits only; opening errs if the WAL ends inside a commit (store.rs's ensure!).
2. Five rules (no field of Rules): spansHeld, spansFenced, acksBetweenCommits, walBetweenCommits, wholeCommitsKept; mutations No* each; existing mutations' Rules gain them.
3. Models BucketSpans (writers, crashes, cuts, WAL) and a collector one; check-tla.sh all; guide pnpm check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-11 (claude): specs/bucket models commits spanning segments, faithful to: State::apply_contents ("Some(begun) if contents.ends(begun.commit)", "Some(begun) if contents.continues(begun.commit)", "Some(_) | None => {}"), Writer::acknowledge (".filter(|(_, pending)| pending.footer.contents.continued.is_none())"), Wal::rebase ("let between ="), extent_store wal::recover ("let ended = |at: usize, commit: u64|") and Recovering::open ("log ends inside a commit"). Can span/cut; rules spansHeld, spansFenced, acksBetweenCommits, walBetweenCommits, wholeCommitsKept; existing mutations' Rules gain them (the guide's evidence lists only their own rule removed).
Mutations, TLC counterexamples: NoSpansHeld (HistoriesOwn: the continuation landed alone, its extent in the branch without its commit); NoSpansFenced (NoErrors: the next writer holds the fenced commit begun past its fence, its store refuses to open); NoAcksBetweenCommits (NothingNeededGone: a checkpoint mid-commit, the collector deletes the continuation's segment, the end lands); NoWalBetweenCommits (HistoriesOwn: after a crash the continuation taken as landed, only the end restamped); NoWholeCommitsKept (NoErrors: a cut mid-commit kept, the store refuses to open).
Models: BucketSpans (2 hosts, crashes, cuts, disk loss; 5,125,798 states, 101 s), BucketSpansCollect (checkpoints, collector, compaction; 636,324 states, 16 s). Full scripts/check-tla.sh: 80 PASS, exit 0. guide: pnpm build + pnpm test green (42 tests).
Found: TASK-345 (Can spanOn, a segment ending one commit and going on with the next, landed alone: Wal::rebase returns Diverged though no other writer moved the branch; the store refuses to open, take drops a locally durable commit). spanOn is in no model until it is fixed; the fix sketched there holds over BucketSpans + spanOn in a scratch copy (5.68M states).
DoD #1: no Rust changed (spec, configs only); nextest not run for it. Note: this host's grep wrapper (~/.local/bin/grep) blocks on a socket stdin, so check-tla.sh run from a backgrounded shell hangs after TLC; run it with < /dev/null.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
specs/bucket now models commits spanning segments (TASK-92.41): continuation segments, readers holding runs until the end and dropping them at a fence, acks and WAL rebase only between commits, crashes cutting kept WALs to whole commits, stores refusing logs that end mid-commit. Five rules, a TLC-failing mutation each; BucketSpans and BucketSpansCollect hold; full check-tla.sh green, guide build/test green. Found TASK-345 (rebase diverges on a landed segment that ends a commit and goes on with the next).
<!-- SECTION:FINAL_SUMMARY:END -->
