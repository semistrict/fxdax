---
id: TASK-42.5
title: >-
  Follower concurrency protocol: seqcount reads, per-inode invalidation, grace
  period before advancing, tested under Shuttle
status: To Do
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-17 22:29'
labels: []
dependencies:
  - TASK-42.8
documentation:
  - backlog/docs/doc-8 - Live-read-only-followers-of-a-mounted-Fxfs.md
parent_task_id: TASK-42
priority: high
ordinal: 125000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
How readers in a follower's guest see whole transactions without a global lock stalling them (doc-8, Concurrency): the kernel's seqlock_t with a lock fallback around tree reads, invalidation of changed inodes under the kernel's own locks, and the kernel's SRCU as a grace period before the view advances. The protocol is Rust, generic over a trait of kernel operations: C shims implement it with the kernel's primitives in the guest, Shuttle's in tests, so Shuttle runs the real code (doc-8 testing 2). No lock-free code of our own with relaxed orderings, which Shuttle (all atomics SeqCst) couldn't check; loom only if that ever changes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The protocol (apply, invalidate, grace period, advance; seqcount and fallback) is Rust behind a small trait of kernel operations, with thin C shims
- [ ] #2 A reader under a busy writer is not starved: the fallback lock bounds retries, shown by a test
- [ ] #3 Shuttle runs the protocol's scenarios: a batch against a reader's tree read, invalidation against a fault that read before the apply and maps after it, advance against a read(2) in flight; check_dfs exhaustively on the smallest, check_pct and check_random over many runs on larger ones (several readers, batches, faults, pin releases); failures replay from their seed
- [ ] #4 Mutation checks: dropping the seqcount check, invalidating before applying, or advancing without the grace period each makes a Shuttle test fail
- [ ] #5 The guest's trait implementation uses only the kernel's seqlock_t, SRCU and locks: no hand-written atomics with relaxed orderings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
