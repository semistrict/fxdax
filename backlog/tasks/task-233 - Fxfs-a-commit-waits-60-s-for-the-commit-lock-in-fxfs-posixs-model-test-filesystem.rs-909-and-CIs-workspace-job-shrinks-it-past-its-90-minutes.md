---
id: TASK-233
title: >-
  Fxfs: a commit waits 60 s for the commit lock in fxfs-posix's model test
  (filesystem.rs:909), and CI's workspace job shrinks it past its 90 minutes
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 16:49'
updated_date: '2026-11-07 17:25'
labels:
  - fxfs
  - bug
dependencies: []
priority: high
type: bug
ordinal: 672000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by CI run 42383533320 (land-h6), whose test (workspace) job was cancelled at its 90-minute limit: fxfs-posix model_tests::whatever_a_vfs_asks_reads_back_as_a_model_says ran from 15:05 to 16:21. Reproduced in Docker (Linux, aarch64), 1 run in 6 of 500 cases, PROPTEST_MAX_SHRINK_TIME=600000: Fxfs panics at crates/fxfs/src/filesystem.rs:909, debug_assert_not_too_long!(self.commit_mutex.lock()): a commit waited 60 s for the commit lock, which something held. Why CI hung rather than failed: each shrink iteration of the failing case waits those 60 s before panicking, which keeps the test's 120 s watchdog (watch_sequences) fed, and proptest shrinks 4 x cases = 384 iterations with no time limit: about 6.4 hours, the failure's report lost to the job's timeout. The case, unshrunk (seed, blocks, ops): (8580878931066154398, 2048, [Make { name: 14, make: Make { kind: 4, permissions: 593185, uid: 4294967294, gid: 1187, rdev: 3649396020 } }, Write { file: 1, at: 31704, len: 23, byte: 243 }, Fill { file: 10, page: 59, pages: 1498, stride: 1, byte: 78 }, FillWhileCompacting { file: 11, page: 39, pages: 21, byte: 148, flushing: false }, Write { file: 3, at: 32768, len: 8151, byte: 208 }, SetAttr { node: 0, change: Change { valid: 20, permissions: 462451, uid: 447, gid: 0, atime: 873050358, mtime: 2931013381, ctime: 0 } }, ReadOn { cursor: 2, count: 0 }, UnlinkLater(9), Create(1), FillWhileCompacting { file: 3, page: 34, pages: 22, byte: 44, flushing: false }, Create(3), SetAttr { node: 0, change: Change { valid: 6, permissions: 461441, uid: 65534, gid: 1320, atime: 0, mtime: 0, ctime: 0 } }, BadName { dir: 0, name: 3 }, Fill { file: 15, page: 55, pages: 3247, stride: 2, byte: 160 }, UnlinkLater(5), FillWhileCompacting { file: 10, page: 5, pages: 18, byte: 168, flushing: true }, Create(8), UnlinkLater(2), Mkdir(2), Close(5), Write { file: 6, at: 48348, len: 4853, byte: 188 }, CloneToEnd { from: 5, to: 0, page: 274877906939, to_page: 30 }, CloneToEnd { from: 13, to: 5, page: 25, to_page: 274877906946 }, Read { file: 8, at: 20480, len: 4096 }, Crash, Truncate { file: 4, size: 110592 }, ReadOn { cursor: 0, count: 1 }, Clone { from: 10, to: 7, page: 21, pages: 5, to_page: 11 }, Write { file: 5, at: 44294, len: 16, byte: 63 }, Open(4), Mkdir(16), Remount, Tmpfile(3), Sync, Punch { file: 1, page: 510, pages: 1682906 }, Punch { file: 14, page: 16, pages: 4 }, Statfs, Make { name: 3, make: Make { kind: 0, permissions: 393722, uid: 0, gid: 65534, rdev: 2048526616 } }, ReadOn { cursor: 3, count: 3 }, Fill { file: 13, page: 37, pages: 24, stride: 2, byte: 145 }, Sync, BadName { dir: 3, name: 0 }]))
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The stall pinned: a test replaying the case (or a smaller one) fails on main
- [ ] #2 Its root cause found and fixed, the pinned test passing
- [ ] #3 A failing model-test case reports within CI's time: shrinking bounded, so a stall's 60 s per iteration can't outrun the job
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce: the model test with shrinking bounded in Docker (found 1 run in 6 of 500 cases); pin the case, minimized from 42 operations to 8.
2. Root cause: instrument the commit lock's holder and waiter.
3. Fix: a sync made while the caller holds the commit lock records no reaches (whose transaction would wait for the lock); the journal's own transaction says so to the allocator's sync; the guest's freeze, which syncs holding commits, too.
4. Shrinking bounded in the model test (SHRINK_FOR_AT_MOST, 10 min unless PROPTEST_MAX_SHRINK_TIME).
5. Verify: pinned test red (60 s stall) and green; 3,000 cases; fxfs, fxfs-posix, follower_sim, fxdax; the guest crate type-checked; CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause (2026-11-07): TASK-213 made every FxFilesystem::sync record what dropped allocations reached (Allocator::record_reaches), in a transaction it commits. A sync can run while its own task holds the commit lock: Allocator::allocate_in syncs when dead extents take the room an allocation needs, and the journal's own transaction allocates as a commit makes room in the journal (commit_transaction holds commit_mutex, Journal::commit, pre_commit, make_room, preallocate_range_in_extents_of, allocate, allocate_in, sync, record_reaches, Transaction::commit, commit_transaction, commit_mutex.lock(): the same task, waiting for itself). Before TASK-213 that nested sync (journal.sync, the device's frees) took no commit lock. In the model test it shows as a 60 s panic (filesystem.rs, debug_assert_not_too_long on the commit lock); in a release guest it is a filesystem hung for ever, as a clone or write commits on a nearly full device with reaches unrecorded. The guest's freeze (fxfs-linux fs.rs) syncs while a task of its holds the commit lock: the same hang if an allocation dropped between its two syncs.
Fix: Transaction::holding_commits (the journal's own, set in make_room), passed by allocate_in to SyncOptions::holding_commits, with which sync records no reaches; they stay queued for the next sync, which records them: a barrier made so seals a shared extent's data its reach isn't recorded for until then (TASK-213's window, which a nested sync on a full device opens, as before TASK-213). The freeze's sync passes it too.
Why CI hung, not failed: each shrink iteration of the case waited the 60 s before panicking, which kept the test's 120 s watchdog fed, and proptest shrinks 4 x cases = 384 iterations with no time limit, ~6.4 h. The model test's runners now shrink 10 min at most (SHRINK_FOR_AT_MOST), unless PROPTEST_MAX_SHRINK_TIME says.
Pinned: a_commit_making_room_in_the_journal_syncs_without_committing (8 operations): red without the fix (60.4 s, filesystem.rs:916's panic), green with it (0.9 s).

Verified: the model test 3,000 cases with the fix in Docker (6 runs of 500, all pass); fxfs, fxfs-posix, follower_sim, fxdax on macOS 1,183 of 1,183; Shuttle part; fxfs-linux type-checked for the guest's target (scripts/build-guest-sysroot.sh, cargo check --release); mutant a-sync-holding-commits-records-reaches (the sync's holding_commits ignored), found by the pinned test in 87 s; upstream-changes.py --check passes.
<!-- SECTION:NOTES:END -->
