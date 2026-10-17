---
id: TASK-42.5
title: >-
  Follower concurrency protocol: seqcount reads, per-inode invalidation, grace
  period before advancing, model-checked with loom and shuttle
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
labels: []
dependencies: []
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 125000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
How readers in a follower's guest see whole transactions without a global lock stalling them (doc-8, Concurrency): a seqcount with a lock fallback around tree reads, invalidation of changed inodes under the kernel's own locks, and an SRCU grace period before the view advances. The protocol is Rust, generic over a trait of kernel operations, so loom and shuttle run the real code (doc-8 testing 2).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The protocol (apply, invalidate, grace period, advance; seqcount and fallback) is Rust behind a small trait of kernel operations, with thin C shims
- [ ] #2 loom explores every interleaving of the core's small scenarios: a batch against a reader's tree read, invalidation against a fault mapping late, advance against a read in flight
- [ ] #3 shuttle runs randomized schedules of larger scenarios (several readers, batches, faults, pin releases), failing schedules replaying from their seed
- [ ] #4 Mutation checks: dropping the seqcount check, invalidating before applying, or advancing without the grace period each makes loom or shuttle fail
- [ ] #5 A reader under a busy writer is not starved: the fallback lock bounds retries, shown by a test
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
