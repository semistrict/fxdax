---
id: TASK-44.5
title: >-
  Object storage protocol (doc-7) in TLA+, the simulation's traces validated
  against it, and cargo-mutants on extent_remote
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 134000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_remote's deterministic simulation (src/sim, TASK-28.1) runs doc-7's protocol (fencing, takeover, forks, collection) over many seeds with rule switches; it samples interleavings, it doesn't exhaust them. A TLA+ spec checks every interleaving at small bounds; validating the simulation's traces against the spec keeps the two the same protocol. The collector has no unit tests of its own.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A TLA+ spec of doc-7's protocol (leases and fencing, takeover, segments and checkpoints, forks, the collector), checked by make check-tla, with one mutation per rule of crates/extent_remote's Rules, each giving a counterexample
- [ ] #2 A sample of the simulation's runs is checked against the spec by TLC trace validation
- [ ] #3 cargo-mutants over extent_remote's protocol code (writer, fetch, collector, checkpoint) leaves no surviving mutant unexplained
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
