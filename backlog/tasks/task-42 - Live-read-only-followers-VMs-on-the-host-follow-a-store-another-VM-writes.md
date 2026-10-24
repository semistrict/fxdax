---
id: TASK-42
title: 'Live read-only followers: VMs on the host follow a store another VM writes'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-17 22:17'
updated_date: '2026-10-24 10:27'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
priority: high
ordinal: 120000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Other VMs on the writer's host mount a store read-only while a VM has it mounted read-write, and follow it live: they see each commit as the writer makes it, a prefix of its history, without remounting, and the writer never waits for them. DAX is the primary path. The design, agreed with the user on 2026-10-17, and above all its testing (concurrency bugs here are silent stale reads, so testing comes first): backlog doc-8. Point-in-time mounts fall out of it (a follower that never advances).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A follower VM mounts a store another VM on the host writes, read-only, and sees each of its commits without remounting, shown by the guest torture test (doc-8 testing 6) passing for hours on GCE, DAX on and off
- [x] #2 Every layer of doc-8's testing exists and catches the bugs it is for: each has a mutation check (the rule it guards turned off makes it fail)
- [ ] #3 The writer's commit latency and pgbench tps are unchanged with followers attached (doc-8 testing 7)
- [x] #4 cargo-mutants (MIT) runs over the follower's crates, and every surviving mutant is killed by a new test or recorded as equivalent, with why (doc-8, Testing)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-24. AC2, each layer of doc-8's testing with its mutation check: 0, TLA+: each of the spec's rules removed (barrier, invalidate, keepFrees, graveyard, cutOff) gives a counterexample (scripts/check-tla.sh); 1, loud failures: extentd's EIO for reads outside a view (extentd tests), and the torture test's Barrier mutant, whose followers' maps of extents let go of extentd refuses; 2, Shuttle: needs_the_barrier, needs_invalidation; 3, the followers' simulation: catches_followers_reading_past_their_views, _splitting_commits, _skipping_changes, needs_flushed_layers_swapped, needs_held_files_kept, needs_changes_told_whole, needs_released_files_let_go; 4, history checking: follower_history's tests of reads past their commits, going back, and held files, and the torture mutants' WRONG histories; 5, comparing each batch: the simulation's compare, which catches skipped changes and unswapped layers; 6, guest torture: the invalidation mutants (DAX, no DAX, the fault window), the barrier and the nested mutants, each caught 3/3 on GCE. AC4: scripts/mutants-followers.sh (cargo-mutants 27.1.0) over the store's views, the Fxfs follower, the protocol and the history checker; tests kill what it left but three equivalent mutants, recorded in doc-8 (Testing). Open: AC1, the hours-long soak (TASK-42.12 found and fixed, running again); AC3, the writer unchanged with followers attached (TASK-42.11, TASK-41.1).

AC1: an hour of the torture test each, with and without DAX, OK history on GCE (2026-10-24, docs/measurements/gce-follow-soak-2026-10-24), after TASK-42.12's fix; two hours in all.
<!-- SECTION:NOTES:END -->
