---
id: TASK-42.5
title: >-
  Follower concurrency protocol: seqcount reads, per-inode invalidation, grace
  period before advancing, tested under Shuttle
status: Done
assignee: []
created_date: '2026-10-17 22:17'
updated_date: '2026-10-18 12:41'
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
- [x] #1 The protocol (apply, invalidate, grace period, advance; seqcount and fallback) is Rust behind a small trait of kernel operations, with thin C shims
- [x] #2 A reader under a busy writer is not starved: the fallback lock bounds retries, shown by a test
- [x] #3 Shuttle runs the protocol's scenarios: a batch against a reader's tree read, invalidation against a fault that read before the apply and maps after it, advance against a read(2) in flight; check_dfs exhaustively on the smallest, check_pct and check_random over many runs on larger ones (several readers, batches, faults, pin releases); failures replay from their seed
- [x] #4 Mutation checks: dropping the seqcount check, invalidating before applying, or advancing without the grace period each makes a Shuttle test fail
- [x] #5 cargo-mutants over the protocol's code leaves no surviving mutant unexplained
- [x] #6 The guest's trait implementation uses only the kernel's seqcount_mutex_t, mutex, SRCU and locks: no hand-written atomics with relaxed orderings
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. crates/follower: Kernel trait (seqcount_mutex_t, mutex, SRCU, per-inode invalidation), Follow trait (fetch: device advanced, journal read, changes; apply), Protocol (read: validated, K tries then the mutex; grace_section; batch: fetch, apply inside the seqcount, invalidate after, grace period, drop previous), Rules (validate, fallback, invalidate_after_apply, grace) settable under shuttle; shuttle_kernel (Shuttle primitives), shuttle_testing (failing runs in child processes). 2. Shuttle tests over the TLA+ spec's tiny filesystem: smallest exhaustively (DFS), mixed and busy randomly and by PCT; each rule's mutation caught; reads counted (at most K tries and the fallback). 3. Fxfs: follow splits into fetch/apply; follower_sim follows through the protocol, readers read through it, no gate. 4. cargo-mutants over the protocol. 5. Guest: C shims implementing Kernel with the kernel's seqcount_mutex_t, mutex, SRCU, invalidate_lock (with TASK-42.6's guest work, after TASK-42.3's device).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Built: crates/follower (lib.rs, shuttle_kernel.rs, shuttle_testing.rs, tests/shuttle.rs, 9 tests: DFS on the smallest in ~15 s once its model was cut to the protocol's own scheduling points; Shuttle's DFS has no partial-order reduction, so every lock and yield of the model multiplies its runs). FxFilesystem::fetch/apply. follower_sim reads alongside batches through the protocol. Findings: the fallback mutation shows as a reader spinning without end under PCT (Shuttle's step bound), the grace mutation needs PCT (a whole batch landing within a copy); a failed Shuttle run's teardown can abort the process (a task woken on a closed lock panics while unwinding), so mutation tests run their failing runs in child processes.

Correction found building the guest (TASK-42.6): the guest's seqcount is the kernel's seqcount_t written with raw_write_seqcount_begin/end holding the follower's mutex, not seqcount_mutex_t, whose write_seqcount_begin disables preemption, which a batch's apply (it sleeps) can't keep. The protocol needs no preemption-disabled writer: its readers try TRIES times, then take the mutex. AC6's intent holds: only the kernel's seqcount_t, mutex, SRCU and locks, no atomics of our own. doc-8 and crates/follower's docs corrected.

Verification (2026-10-18): AC1 crates/follower (Protocol, Kernel, Follow) with the guest's C shims (fs/fxfs/super.c fxfs_vfs_follow_*, GuestKernel in fxfs-linux's follow.rs), pushed c0c0f67. AC2 busy_randomly/busy_by_pct count tries (at most TRIES, then the mutex); reads_without_batches_take_no_lock. AC3 smallest_exhaustively (DFS), mixed/busy random and PCT; pin releases run through the protocol in follower_sim (Fxfs's graveyard pins); Shuttle prints each failing schedule to replay. AC4 needs_reads_validated, needs_invalidation_after_the_apply, needs_the_grace_period, needs_the_fallback. AC5 cargo-mutants over crates/follower: the two survivors (a fallback always taken; the invalidation's two sites) killed by 4d2cbda. AC6 seqcount_t written with raw_write_seqcount_* under the follower's mutex (seqcount_mutex_t's writer disables preemption), mutex, SRCU, invalidate_lock: no atomics of our own. DoD: macOS full suite (2039) and GCE x86-64 (2085, Shuttle suites; the project's Linux runs are on GCE now, not Lima); upstream-changes.py --check passes; the deviation (seqcount_t) documented in vfs.h, follow.rs, crates/follower and doc-8. TLC trace validation of the protocol's runs (TASK-42.1) matches them against the spec.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The followers' concurrency protocol is Rust (crates/follower): validated tree reads falling back to the follower's mutex, invalidation after the apply, a grace period before letting go; Shuttle runs it (DFS, random, PCT), each rule's removal fails a test, cargo-mutants leaves nothing unexplained, and TLC trace-validates its runs against the TLA+ spec. The guest implements its kernel trait with the kernel's seqcount_t (written under the mutex: seqcount_mutex_t's writer disables preemption, and a batch sleeps), mutex and SRCU; fs/fxfs follows its writer through it (TASK-42.6). Verified on macOS and GCE x86-64; guest follow tests pass.
<!-- SECTION:FINAL_SUMMARY:END -->
