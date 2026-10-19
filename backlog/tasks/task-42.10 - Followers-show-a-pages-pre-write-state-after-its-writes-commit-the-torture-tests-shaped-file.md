---
id: TASK-42.10
title: >-
  Followers show a page's pre-write state after its write's commit: the torture
  test's shaped file
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-19 05:48'
updated_date: '2026-10-19 11:33'
labels:
  - followers
  - dax
dependencies: []
parent_task_id: TASK-42
priority: high
ordinal: 145000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The followers' history checker (TASK-42.7, follower_history) finds, in fxfs-guest-follow-torture and -dax on GCE, a follower's reader of the shaped file (a file the writer truncates, extends and rewrites a page of each step) reading a page as zeros, the state after an extension and before the write, at commits after the write's (its sync's epoch) and after that commit's batch was invalidated (FXFS_IOC_FOLLOW_EPOCHS's settled). Run kc-records 2, DAX: page 31 extended (model 2144, by epoch 80), written generation 302 (model 2173, by 81), truncated (model 2270, after 83): neither follower ever read 302; follower2 read zeros at commit 80, follower1 zeros at 83..84. Non-DAX, run 1: page 214 extended (by 39), written 136 (by 39), truncated (after 39): follower2 read zeros at 39..40, follower1 read 136 once. The pattern needs the file's size to change around the write; read(2) of the blocks file, rewritten in place, never shows it. Two followers agreeing points at the store's view of the commit or the writer's path rather than one follower's caches. The records the host keeps (history-ROLE.txt beside the consoles) and scratch analysis (each operation on a name with its model index and epoch bounds) reproduce the findings.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The mechanism by which a follower shows the page's pre-write state after the write's commit is identified, with evidence
- [ ] #2 Fixed, or shown to be the history model's error (e.g. the writer's operations reaching its journal in another order than its syscalls), with the model or the workload corrected
- [ ] #3 fxfs-guest-follow-torture and -dax report OK history in 10 runs of 10 on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. The user's decision (2026-10-19): the fill barrier, the changed files' caches dropped before the apply, fills waiting for the batch; the seqcount, the mutex fallback and SRCU deleted with their mutation tests (Shuttle needs_reads_validated, needs_the_fallback, needs_the_grace_period, needs_invalidation_after_the_apply; TLA+ NoValidate, NoFallback, NoGrace, NoGraceFirst) and the torture test's FaultLock mutant, replaced by needs_the_barrier, needs_invalidation, NoBarrier (+Torn, +Copy), NoInvalidate and the torture test's Barrier mutant.
2. Spec: readers' operations hold the barrier; loads through mappings; NoGoingBack; a batch waits for the barrier, drops, applies; trace spec and models.
3. crates/follower: Kernel (fill_begin/end, barrier_take/release, drop_cached, settle), Protocol::fill and batch; the Shuttle kernel's rwsem, its writer waiting first; Shuttle tests and traces; follower_sim's readers fill.
4. fs/fxfs: the barrier on every entry point that reads the trees; read(2) in 1 MiB chunks, copying without page faults (fault in, retry); fault wrappers for the page cache; batches take no i_rwsem (order: i_rwsem, barrier, invalidate_lock); drop and settle split; FXFS_IOC_FOLLOW_STATS (the batches' waits); mutant knobs follow_skip_barrier, follow_read_faults.
5. Guest Rust: follow.rs and fs.rs on the new trait; tree reads assert the barrier (lockdep).
6. Torture: a reader read(2)ing into a private mapping of the blocks' file (nested fills), the Barrier and Nested mutants, the barrier waits printed.
7. doc-8.
8. Verify: TLC, all models, mutations and traces; Shuttle; follower_sim; debug and release guest kernels on GCE: follow, torture 10/10 OK history, mutants caught.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Mechanism (GCE, torture records and in-guest probes, 2026-10-19): a batch applies its commits to the follower's tree, then invalidates each changed file. Between the two, a reader's fault, readahead or read(2) of a page no cache holds reads the new tree, while pages it already has cached or mapped stay old until the invalidation reaches them. So one pass over a file sees a mix no commit had. kc-inval run 1, non-DAX: reader 19 read the shaped file's pages 0, 1, 3, 13, 20 and 23 at commit 441 (generations 1733-1744) and then pages 30 and 31 at 428 (1678, 1679), in one pass, during batch 441, whose invalidation of inode 6 (0..819200, size 970752 to 593920) came after. read(2) right after a stale mapped read returns the new page; every follower converges at the end (done.shape), so nothing is lost. DAX alike: faults map new extents while other PTEs are stale. The spec (specs/follower/Follower.tla) checks a reader never goes back within a pair read, not across a reader's faults and loads through mappings it already has, so TLC passed it. The mechanism is the protocol's (doc-8, Concurrency): invalidate-after-apply, with readers unexcluded from the window. A fix: a fill barrier (an rwsem a follower's cache fills take shared, a batch exclusive from before it applies until its invalidation is done), with the changed files' caches dropped before the apply. It must be designed around the dcache: lookup and readdir run with the directory's i_rwsem held by the VFS, while a batch changing the directory would take the barrier and then that lock. It also changes the spec (a load action, never-going-back across a reader's operations), the trace validation (whose mutant is invalidate-first), the follower crate's Kernel trait and Shuttle model, and doc-8. That is more than a day; recorded here, with the user's say on the design (doc-8 decision).

Built (2026-10-19), as the user decided: the fill barrier, the changed files' caches dropped before the apply; the seqcount, the mutex fallback and SRCU deleted with their mutation tests, and the torture test's FaultLock mutant (all approved). TLC: every model holds (Follower 5.0M states, FollowerReaders 2.0M, FollowerLoads 0.9M, FollowerLive, FollowerStalled) and every mutation is caught (NoBarrier, NoBarrierTorn, NoBarrierCopy, NoInvalidate, NoGraveyard, NoKeepFrees, NoCutOff); trace validation 20/20 runs matched, 5/5 mutant runs not. Shuttle 8/8 (needs_the_barrier, needs_invalidation), follower_sim 16/16.

GCE, dax10: debug kernel (lockdep, KCSAN), once: follow, follow-dax, torture-dax, the Invalidation and Nested mutants passed, no complaint; non-DAX torture failed on a KCSAN report in virtqueue_get_buf_ctx_split (the host's write of the used ring, reviewed and added to KCSAN_REVIEWED). Release kernel, 10 runs (kc-barrier3): fxfs-guest-follow, -dax, -torture, -torture-dax OK 10/10 (OK history); the Barrier mutant: both followers' VMMs abort on a DAX window fault extentd refuses (an extent released under a reader without the barrier), and the history is WRONG, 10/10, which its expected output now says. An earlier run found an open of a file a batch deleted after its lookup holding nothing and reading ENOENT: fixed, the open fails ENOENT (fxfs_hold returns it; Fxfs::hold checks the object under the barrier).

Barrier waits (FXFS_IOC_FOLLOW_STATS, torture, release kernel): non-DAX, 160-210 ms over ~770 batches, 8 ms at most, readers as fast as before (~200k reads checked); DAX, 9.8-13.3 s over 430-590 batches, 22 ms on average, 41 ms at most, readers 2 to 8 times slower (7.5k-33k reads checked against 57k-67k before): faults remap what a batch dropped holding the barrier. Not looked into further: the user paused the follower work (2026-10-19).

Last change, unbuilt (GCE stopped by the user's priority call): a follower's inodes get operation tables of their own (getattr, read_iter, mmap, fadvise, splice_read under the barrier); a writer's are as before the barrier, per the user's rule that followers cost a writer nothing when not enabled. The writer-alone benches (AC5 of TASK-42.7, clean builds, 2026-10-19) predate the barrier; they need rerunning on this tree.
<!-- SECTION:NOTES:END -->
