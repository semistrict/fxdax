---
id: TASK-42.1
title: >-
  Followers' test foundation: deterministic simulation of writer, follower and
  readers, and a history checker
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 08:35'
labels: []
dependencies:
  - TASK-42.8
  - TASK-42.2
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 121000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Testing comes first for followers (doc-8, Testing): a follower's bugs are silent stale reads that need rare interleavings, so the harnesses that make them reproducible are built before the follower they test, and the follower is built against them. The simulation runs on Shuttle (doc-8 testing 2-3; chosen over loom on 2026-10-17): our fuchsia-async and fuchsia-sync get a shuttle feature backed by Shuttle's futures and primitives, so Fxfs runs unchanged under its scheduler. Model on crates/extent_remote/src/sim (TASK-28.1). To find out first: what in Fxfs runs on threads of its own or blocks outside fuchsia-sync, which then needs the shuttle feature too.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A history checker requires each reader operation's result to equal the writer's state at some commit between those applied at its start and end, and each reader's commits never to go back; the oracle is a store view of each commit
- [x] #2 After each batch, a comparison walks the follower's trees against the writer's as of that commit (objects, attributes, entries, extents)
- [x] #3 Mutation checks: a follower that applies half a transaction, skips a change, or reads past its view makes the simulation fail within the default seeds
- [x] #4 It runs on macOS within cargo nextest run, in seconds
- [x] #5 A deterministic simulation runs the real Fxfs as writer and follower on one store, with many reader tasks, under Shuttle's scheduler (fuchsia-async and fuchsia-sync backed by Shuttle with a shuttle feature); a failing seed or schedule replays exactly; SIM_SOAK multiplies the runs
- [ ] #6 A sample of the simulation's histories is checked against the TLA+ spec (TASK-42.8) by TLC trace validation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. fuchsia-sync and fuchsia-async get a shuttle feature (Shuttle's locks via shuttle-parking_lot; a futex of Shuttle's primitives for the condvar; tasks/executors/timers on shuttle::future, timers never firing) so Fxfs runs unchanged under Shuttle. 2. crates/follower_sim: Fxfs devices over an extent store (StoreDevice) and a view (ViewDevice). 3. The simulation: writer ops, a follower (remounting at each view until TASK-42.4), readers; history checker over per-transaction models bounded by commits; per-commit tree comparison against an independent mount; final check; reach counts; mutations. 4. TLC trace validation once the incremental follower (42.4) has batches to trace.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built: fuchsia-sync/fuchsia-async shuttle features; follower_sim (device.rs, model.rs, history.rs, sim.rs, probe.rs). Found: Fxfs outside its tests runs only on fuchsia-async/fuchsia-sync, so the swap puts all of it under Shuttle; the kernel (zircon, through zx) is in every Fxfs build and Cargo unifies the parking_lot wrapper's feature, so its shuttle feature comes along. Store views had to refuse extents sealed after their commit (Fxfs's journal replay reads on until NoExtent, which reads as zeros): State.sealed records sealing epochs while views live, checked in the same critical section as the read (Reader::View); the Shuttle harness's follower caught the first version's check-then-read race. Mutations caught: ReadsPastItsView (by the per-commit comparison), SkipsChangesTo, SplitsCommits (by the history checker). SIM_SOAK=10 clean. AC6 (TLC trace validation) waits for 42.4: the remounting follower has no batches to map onto the spec's follower actions.

Pushed to main (7f933fa..7d66ead). GCE found two flakes, both fixed: the decide_at_publish mutation also tripped the sealed-extent check (fe8104f), and Fxfs under Shuttle on Linux died 11/40 when the zircon fault handler, waiting on a Shuttle lock with SIGSEGV blocked, let another coroutine fault (7d66ead, SA_NODEFER under shuttle; 0/40 after). AC6 stays open until 42.4's follower has batches to trace.
<!-- SECTION:NOTES:END -->
