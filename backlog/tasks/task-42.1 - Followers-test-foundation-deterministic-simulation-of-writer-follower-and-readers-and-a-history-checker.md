---
id: TASK-42.1
title: >-
  Followers' test foundation: deterministic simulation of writer, follower and
  readers, and a history checker
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:29'
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
- [ ] #1 A history checker requires each reader operation's result to equal the writer's state at some commit between those applied at its start and end, and each reader's commits never to go back; the oracle is a store view of each commit
- [ ] #2 After each batch, a comparison walks the follower's trees against the writer's as of that commit (objects, attributes, entries, extents)
- [ ] #3 Mutation checks: a follower that applies half a transaction, skips a change, or reads past its view makes the simulation fail within the default seeds
- [ ] #4 It runs on macOS within cargo nextest run, in seconds
- [ ] #5 A deterministic simulation runs the real Fxfs as writer and follower on one store, with many reader tasks, under Shuttle's scheduler (fuchsia-async and fuchsia-sync backed by Shuttle with a shuttle feature); a failing seed or schedule replays exactly; SIM_SOAK multiplies the runs
- [ ] #6 A sample of the simulation's histories is checked against the TLA+ spec (TASK-42.8) by TLC trace validation
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
