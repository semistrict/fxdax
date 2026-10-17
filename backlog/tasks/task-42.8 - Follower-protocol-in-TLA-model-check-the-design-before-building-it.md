---
id: TASK-42.8
title: 'Follower protocol in TLA+: model-check the design before building it'
status: To Do
assignee: []
created_date: '2026-10-17 22:29'
updated_date: '2026-10-17 22:29'
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
- [ ] #1 A TLA+ spec (in the repo, with its TLC configurations) models the writer (transactions, commits, frees), store views (epochs, kept frees, pins, cut-off), the follower (advance, batch apply, seqcount, per-inode invalidation, grace period, graveyard) and readers (tree reads with retry and fallback, faults mapping extents, read(2) in flight, files deleted while open)
- [ ] #2 TLC checks the safety properties over small bounds: every read sees a committed prefix of the writer's history, readers' prefixes never go back, no read touches an extent released, no mapping of an old extent outlives the follower's advance, a file deleted while open reads to its end
- [ ] #3 TLC checks liveness under fairness: the writer is never blocked by a follower, a follower advances, readers are not starved, pins are released after close or advance, and a lagging follower is cut off rather than pinning without bound
- [ ] #4 Mutation checks: removing each rule (validation after reads, invalidation after apply, the grace period, keeping frees for older views, graveyard pins) gives a TLC counterexample
- [ ] #5 Running it is one script, documented in doc-8
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
