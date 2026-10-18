---
id: TASK-42.5
title: >-
  Follower concurrency protocol: seqcount reads, per-inode invalidation, grace
  period before advancing, tested under Shuttle
status: In Progress
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 10:35'
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
How readers in a follower's guest see whole transactions without a global lock stalling them (doc-8, Concurrency): the kernel's seqcount_mutex_t (not seqlock_t, whose writer holds a spinlock that applying Fxfs's mutations, which may sleep, can't: found by the TLA+ spec, TASK-42.8) with the follower's mutex as the fallback around tree reads, invalidation of changed inodes under the kernel's own locks, and the kernel's SRCU as a grace period before the view advances. The protocol is Rust, generic over a trait of kernel operations: C shims implement it with the kernel's primitives in the guest, Shuttle's in tests, so Shuttle runs the real code (doc-8 testing 2). No lock-free code of our own with relaxed orderings, which Shuttle (all atomics SeqCst) couldn't check; loom only if that ever changes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The protocol (apply, invalidate, grace period, advance; seqcount and fallback) is Rust behind a small trait of kernel operations, with thin C shims
- [ ] #2 A reader under a busy writer is not starved: the fallback lock bounds retries, shown by a test
- [ ] #3 Shuttle runs the protocol's scenarios: a batch against a reader's tree read, invalidation against a fault that read before the apply and maps after it, advance against a read(2) in flight; check_dfs exhaustively on the smallest, check_pct and check_random over many runs on larger ones (several readers, batches, faults, pin releases); failures replay from their seed
- [ ] #4 Mutation checks: dropping the seqcount check, invalidating before applying, or advancing without the grace period each makes a Shuttle test fail
- [ ] #5 cargo-mutants over the protocol's code leaves no surviving mutant unexplained
- [ ] #6 The guest's trait implementation uses only the kernel's seqcount_mutex_t, mutex, SRCU and locks: no hand-written atomics with relaxed orderings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/follower: Kernel trait (seqcount_mutex_t, mutex, SRCU, per-inode invalidation), Follow trait (fetch: device advanced, journal read, changes; apply), Protocol (read: validated, K tries then the mutex; grace_section; batch: fetch, apply inside the seqcount, invalidate after, grace period, drop previous), Rules (validate, fallback, invalidate_after_apply, grace) settable under shuttle; shuttle_kernel (Shuttle primitives), shuttle_testing (failing runs in child processes). 2. Shuttle tests over the TLA+ spec's tiny filesystem: smallest exhaustively (DFS), mixed and busy randomly and by PCT; each rule's mutation caught; reads counted (at most K tries and the fallback). 3. Fxfs: follow splits into fetch/apply; follower_sim follows through the protocol, readers read through it, no gate. 4. cargo-mutants over the protocol. 5. Guest: C shims implementing Kernel with the kernel's seqcount_mutex_t, mutex, SRCU, invalidate_lock (with TASK-42.6's guest work, after TASK-42.3's device).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built: crates/follower (lib.rs, shuttle_kernel.rs, shuttle_testing.rs, tests/shuttle.rs, 9 tests: DFS on the smallest in ~15 s once its model was cut to the protocol's own scheduling points; Shuttle's DFS has no partial-order reduction, so every lock and yield of the model multiplies its runs). FxFilesystem::fetch/apply. follower_sim reads alongside batches through the protocol. Findings: the fallback mutation shows as a reader spinning without end under PCT (Shuttle's step bound), the grace mutation needs PCT (a whole batch landing within a copy); a failed Shuttle run's teardown can abort the process (a task woken on a closed lock panics while unwinding), so mutation tests run their failing runs in child processes.
<!-- SECTION:NOTES:END -->
