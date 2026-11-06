---
id: TASK-44.5
title: >-
  Object storage protocol (doc-7) in TLA+, the simulation's traces validated
  against it, and cargo-mutants on extent_remote
status: To Do
assignee: []
created_date: '2026-10-18 04:06'
updated_date: '2026-11-06 00:55'
labels: []
dependencies: []
parent_task_id: TASK-44
priority: medium
ordinal: 134000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
extent_remote's deterministic simulation (src/sim, TASK-28.1) runs doc-7's protocol (fencing, takeover, forks, collection) over many seeds with rule switches; it samples interleavings, it doesn't exhaust them. A TLA+ spec checks every interleaving at small bounds; validating the simulation's traces against the spec keeps the two the same protocol.

At dde8d58d3 the spec exists: specs/bucket (TASK-165.2, 25c2b6314, with TASK-175 to 180 and 183's rules since), doc-7's protocol with doc-14's WAL: leases with tokens, recovery, fencing, segments and checkpoints, forks, points and restores, failed forks' deletions, one collector compacting and deleting; eleven models as designed and a mutation per field of extent_remote's Rules (crates/extent_remote/src/lib.rs:122-253), each failing as its configuration says (specs/bucket/Bucket.tla:46-136), checked by scripts/check-tla.sh. That is AC #1. The collector has unit tests of its own now (crates/extent_remote/src/collector.rs:659-). Left: trace validation of the simulation's runs against specs/bucket, as crates/follower's Shuttle runs are validated against specs/follower (check-tla.sh's follower/traces); and cargo-mutants over the protocol code (TASK-44.1's notes have extent_remote's sweep of 2026-10-19, before the WAL and TASK-165.3's one replay).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A TLA+ spec of doc-7's protocol (leases and fencing, takeover, segments and checkpoints, forks, the collector), checked by make check-tla, with one mutation per rule of crates/extent_remote's Rules, each giving a counterexample
- [ ] #2 A sample of the simulation's runs is checked against the spec by TLC trace validation
- [ ] #3 cargo-mutants over extent_remote's protocol code (writer, fetch, collector, checkpoint) leaves no surviving mutant unexplained
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: AC #1 met by specs/bucket (TASK-165.2, 25c2b6314; TASK-175 to 180 and 183's rules since). check-tla.sh is a script, not a make target. Verified on main dde8d58d3, macOS: scripts/check-tla.sh over specs/bucket's 11 models and 32 mutations, 43 of 43 (the models hold, each mutation violates what its configuration names), 17 min. Left: AC #2 (trace validation), AC #3 (cargo-mutants on extent_remote).
<!-- SECTION:NOTES:END -->
