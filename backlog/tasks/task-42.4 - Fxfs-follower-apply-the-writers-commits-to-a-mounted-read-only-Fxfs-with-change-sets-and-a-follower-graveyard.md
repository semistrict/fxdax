---
id: TASK-42.4
title: >-
  Fxfs follower: apply the writer's commits to a mounted read-only Fxfs, with
  change sets and a follower graveyard
status: Done
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 10:40'
labels: []
dependencies:
  - TASK-42.1
  - TASK-42.2
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 124000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The follower itself (doc-8, The follower in Fxfs): after a read-only mount on a view, it applies each later transaction to the live trees as the writer's own commits apply theirs, and says what changed so the kernel can invalidate. Built and tested in the simulation (TASK-42.1) before any guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A follower applies flushes (EndFlush), journal moves to new extents (a new super-block), deletions (graveyard, tombstones) and per-object cached state, shown by the per-batch comparison over many seeds
- [x] #2 Each batch yields a change set: attributes changed, directory entries changed, data ranges changed, objects deleted
- [x] #3 A file deleted while a follower's reader has it open reads to its end until closed (the follower's graveyard pins its extents), then its pins go
- [x] #4 The simulation and history checker pass over SIM_SOAK seeds with mutation checks for each rule
- [x] #5 cargo-mutants over the follower's code leaves no surviving mutant unexplained
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. storage_device::Device: advance() (to the writer's latest commit, returning what keeps the previous commit's frees until dropped), pin/unpin (the follower's graveyard). follower_sim's ViewDevice implements them over extent_store views. 2. Fxfs journal: read_transactions returns where its last whole transaction ends (checkpoint); a read-only mount keeps its journal reader there. 3. FxFilesystem::follow(): advance the device, read the journal on, apply each whole transaction (root parent, root, other stores; the allocator's are left: a follower allocates nothing), at EndFlush swap the store's layers for those its new StoreInfo lists, and return the batch's Changes (attributes, entries, data ranges, deletions). 4. Follower graveyard: FxFilesystem::hold(store, object) -> Hold; a deletion (graveyard entry or tombstone) of a held object keeps its records (skipping its mutations, re-inserted after a layer swap) and pins its extents; after the last Hold drops, the next follow applies what it skipped and unpins. 5. follower_sim: an incremental follower mode (mount once, follow), the per-batch comparison, a writer op that compacts (flushes, new super-block), checks that each batch's Changes cover what changed, a reader holding files the writer deletes; mutations: skipping EndFlush layer swaps, change sets missing changes, graveyard not keeping. 6. cargo-mutants over the follower's code; doc-8 updated.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built: storage_device Device::{advance (returns Previous, keeping the previous commit's frees until dropped), pin, unpin}; Journal::read_on (read_transactions returns the end of the last whole transaction; a read-only mount that replayed everything keeps its reader there; JournalReader::rewind); ObjectStore::take_flushed_layers (flush.rs); crates/fxfs/src/follower.rs: FxFilesystem::follow (advance, read on, release holds let go of, apply each transaction store by store in ApplyMode::Replay, allocator's left, EndFlush swaps layers and re-inserts kept records) and FxFilesystem::hold (Hold); Changes; Rules (swap_flushed_layers, keep_held, tell_data, apply_released) settable under fxfs's shuttle feature. Decisions: Replay mode rather than Live (no Transaction, no reservations or object-ID bookkeeping on a follower); the allocator's mutations are skipped (a follower allocates nothing; its free space figures stay as mounted); a kept object reads as before its unlink (nlink and ctime from before; the change set says it was deleted). follower_sim: the follower follows incrementally; per batch, trees, layers and root stores' records compared with an oracle mount, change sets checked; holders hold files the writer unlinks (purged in the unlink's transaction or after); final batch lets go of all kept and the view pins nothing. Mutations caught: each Rules member, ReadsPastItsView, SkipsChangesTo, SplitsCommits. Sim bugs found on the way: the held set snapshotted before the gate, and 'previous' set to the new mount.

Verification (2026-10-18): macOS scripts/test.sh: 2025 workspace, extent_store 12+1, fxdax 9, follower 9, follower_sim 23 Shuttle tests, scripts OK; GCE (fxdax-test-views, the user's rule: Linux on GCE, not Lima): 2065 workspace, 12+1, 15, 9, 23, OK. upstream-changes.py --check passes. cargo-mutants over follower.rs (tested by follower_sim): every mutant caught; read_on and take_flushed_layers' mutants caught; field-deletion mutants of upstream code the run's filters let through are of unrelated code. Change sets are checked per batch against what changed in the follower's tree (entries, attributes, data ranges covering what changed, deletions).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A read-only Fxfs mount follows its writer (crates/fxfs/src/follower.rs): fetch advances the device (Device::advance, keeping the previous commit's frees until dropped) and reads the journal on from the end of the last whole transaction (Journal::read_on); apply applies each transaction as replay does, store by store, the allocator's left, a flush's end swapping the store's layers for those it wrote (take_flushed_layers); each batch's Changes say what to invalidate. The follower's graveyard (FxFilesystem::hold) keeps a held object's records and pins its extents when its writer deletes it, until the hold goes. Verified by follower_sim: the incremental follower compared batch by batch with an oracle mount (trees, layers, records), change sets checked, holders' reads checked by the history checker, deterministic graveyard cases (purges at once, through the graveyard, over several transactions; truncation), mutation tests for each of the follower's rules, every cargo-mutants mutant caught; macOS and GCE suites green.
<!-- SECTION:FINAL_SUMMARY:END -->
