---
id: TASK-371
title: >-
  A durable sync takes 12 bucket round trips where one would do: sync(2) makes 3
  durable commits, each waits for 2 upload cycles, each a segment PUT then a
  lease GET, in series (3/s at 25 ms latency)
status: Done
assignee:
  - '@claude'
created_date: '2026-11-12 01:53'
updated_date: '2026-11-13 10:19'
labels:
  - bug
  - extentd
  - performance
dependencies:
  - TASK-369
priority: high
type: bug
ordinal: 754000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
ubu1, 2026-11-12, branch sync-why (e48bfc57: the sync loops observed, REQUESTS/FXFS lines), sync group alone, file bucket with FXDAX_REMOTE_LATENCY_MS=25, bench-fs-fxfs-guest-dax (durable): sync-loop 3.2/s, syncfs 2.8/s, freeze 2.2/s, ~330 ms a round for a 4 KiB write. With one object-storage round trip of 25 ms the floor is ~35-40/s.

Guest side, per sync(2) round: 2 WRITEs and 2 COMMITs reach the device, 3 of them durable commits (FXFS device_durable_commits 150 for 50 rounds; one WRITE carries its durable commit, barriers.after): write 2.00 (50 ms each, the average of one ~0 and one ~108), commit 2.00 (104 ms each). sync(2) calls sync_fs twice (wait 0, then 1), and fxfs_vfs_sync_fs ignores wait: both a full fxfs_sync. syncfs likewise.

Daemon side (extentd-stats.txt): commits=509, commit-wait-ns=55.45 s, ~109 ms a durable commit. object-store create=518 at ~30 ms each (segment PUTs, one per commit, segment-closes=505) and get=511 at ~26 ms each: the lease check of every acknowledgement (extent_remote writer.rs, Writer::acknowledge, branch::holds_lease when rules().acks_after_lease), after the PUT, not beside it. One PUT plus one GET is ~56 ms; a durable commit waits ~109 ms, two such cycles: why the commit's epoch needs a second upload is not known yet.

So 3 commits x 2 cycles x 2 serial requests = 12 round trips.

The same run with cache_type Unsafe (bench-fs-fxfs-guest-dax-unsafe) does 3,470 syncs/s, the device's requests 30-50 us each: the cost is all in waiting on the bucket.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A durable commit with nothing new since the last durable one returns without an upload, or the guest sends none: sync(2) costs one durable commit, not three
- [x] #2 A durable commit waits for one upload cycle, not two
- [x] #3 The lease check of an acknowledgement goes beside the segment's PUT, or a lease held by time needs none: one round trip a durable commit
- [x] #4 bench-fs-fxfs-guest-dax's sync-loop on ubu1 at 25 ms bucket latency: 25/s or more
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Guest: one durable commit per sync. Fxfs::sync (fxfs-posix) holds the durability guard only over its final device flush, the journal's flushes ordered; fxfs_vfs_sync_fs does nothing for wait=0 (the VFS always asks again waiting) nor while the volume is freezing; freeze_fs makes it durable once, by the filesystem's freeze or, for a volume not the last to freeze, a sync of its own; Filesystem::freeze's two syncs ordered, then one durable flush.
2. Protocol: acknowledge without reading the lease, the creates the fencing (doc-7 decision 4's explicit takeover kept). Two collector rules make that safe, found and checked in specs/bucket's TLA+: fences_kept (State::fences, the first segment of each epoch, kept for live branches; FORMAT 4 as checkpoints carry them) and takeovers_kept (all of a live branch's segments while its lease epoch is past its state's). forks_after_lease keeps the lease read for segments holding forks (deleted branches' fences go). acks_after_lease removed.
3. Tests: fxfs-posix syncs_make_one_commit_durable; extent_remote tests replaying each TLA+ trace with its rule on and off; fxdax durable_commits_cost_their_segments_create (1 create, 0 gets); sim needs-tests per rule; TLA+ mutations NoFencesKept, NoTakeoversKept, NoForksAfterLease.
4. Measure on ubu1: bench-fs-fxfs-guest-dax sync group at 25 ms.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Found (2026-11-12, ubu1, durable sync loop alone, branch sync-why e48bfc57): per sync(2), 2 WRITEs + 2 COMMITs, 3 of them durable (one WRITE carries its commit). Each durable commit = 1 segment create + 1 lease GET, in series (crates/fxdax remote.rs's durable_commits_cost_their_segments_create_and_a_lease_read pins exactly that), ~63 ms of bucket at 25 ms + local. Locally (macOS, file bucket, FXDAX_REMOTE_LATENCY_MS=25, bucket_round_trips_by_phase): durable-commit 70-83 ms, get 1 + create 1. fxfs-posix: 2 durable barriers per Fxfs::sync before, 1 after. The 'two upload cycles' guess in the description was wrong: one cycle of two requests, plus ~10-40 ms local.
TLA+: fences kept alone isn't enough: a taker that read the branch before the last writer's segment landed fences where the collector deleted it after the last writer acknowledged and checkpointed it (TLC trace; extent_remote test writers_taking_over_finding_what_landed_after_they_read_need_takeovers_kept reproduces it in code, through the taker's segments listing). Hence takeovers_kept. With it, newest_checkpoint's writer case is covered too: NoNewestCheckpoint now removes both; NoNewestCheckpointInherited still shows the rule needed alone.

Measured (ubu1, 2026-11-12, tree 6df0a95a0 = this change before its rebase, guest kernel from it, sync group alone, FXDAX_REMOTE_LATENCY_MS=25, results ~/src/fxdax-t350-results/t371):
- bench-fs-fxfs-guest-dax (durable): sync-loop 3.2/s -> 15.9/s; syncfs 2.8 -> 7.1/s; freeze 2.2 -> 2.1/s. Durable device commits per round (FXFS device_durable_commits / 50): sync 3 -> 1, syncfs 3 -> 1, freeze 3 -> 2. Bucket over the whole run: get=6 (was 511: no lease reads), create=261 at 36.7 ms each, one per durable commit. A durable sync's commit took 62 ms at the device, syncfs's 140 ms, freeze's 159 ms: past the 37 ms create, the rest is local, the store's flush. That run was contended: fxdax-bench.slice stalled 43.9 s on CPU and 49.9 s on IO (runs.tsv), the Unsafe run after it 0.02 s and 0.5 s; TASK-369's traces put a flush's fdatasyncs behind ext4 journal commits under contention.
- bench-fs-fxfs-guest-dax-unsafe: sync 4,351/s, syncfs 3,712/s, freeze 548/s (were 3,471, 3,255, 507).
- Locally (macOS, file bucket at 25 ms, bucket_round_trips_by_phase): a durable commit's median 75.7 -> 45.1 ms, get 1 -> 0.
AC #4 (25/s) not met: the bucket part is one round trip; what remains is the local flush (TASK-369, TASK-322's queue), and the run's contention. To re-measure in a quiet window and once TASK-369 lands.
Checks: extent_store 242/242 (sim needs-tests: fences_kept seed 0 of recreated(), takeovers_kept seed 1702 of late_checkpoints(), forks_after_lease of deleted_bases(); each loses an acknowledged commit, or a child's segments, without its rule), extent_remote 210, fxfs-posix 102, fxdax remote:: 68, TLA+ all 83 models (scripts/check-tla.sh), scripts' Python 305, guide's anchors check.

Re-run on main 3037d0772, bench lock, IO cap in force (2026-11-12 09:43Z, ~/src/fxdax-t350-results/t371b): bench-fs-fxfs-guest-dax sync 6.1/s, syncfs 8.3/s, freeze 3.3/s; CPU stall 0.05 s, IO stall 6.4 s. Still one durable commit a sync and 6 bucket GETs the whole run, creates 36 ms each; the daemon waits ~145 ms a durable commit. Where the rest goes (78b94e218, durable_commits_through_the_daemon, a guest round's requests through the daemon at 25 ms): median 37.0 ms with the store on ubu1's ZFS home (one round trip + 12 ms, ~27 syncs/s), 116.9 ms (76-543) on /mnt/fxdax-scratch, ext4 on a zvol, where the bench's stores are; the store alone there (bucket_round_trips_by_phase, no writes between commits) 40 ms. So the writes' growing raw files make the flush's fdatasyncs wait for ext4 journal commits: TASK-369's finding #1. AC #4 waits on that (dependency TASK-369).

2026-11-13, AC #4 on ubu1, main 10f613290, bench-fs-fxfs-guest-dax FXDAX_BENCH_GROUPS=sync at FXDAX_REMOTE_LATENCY_MS=25, one round each, ubu1 idle (fxjob ps: 0 jobs):
- store on ext4 scratch (/mnt/fxdax-scratch), bucket on /dev/shm: sync-loop 30.1/s (32.5 ms a durable commit, the bucket's round trip ~30 ms), syncfs-loop 30.8/s, freeze-loop 9.9/s.
- bench-rounds.sh's default, store and file bucket both on ZFS home: sync-loop 10.8/s (91 ms), syncfs 13.1/s, freeze 3.7/s. Bucket on /dev/shm, store still on ZFS: 15.9/s (62 ms).
The 25 ms bucket costs one round trip a sync, as asked; what's left in the default layout is ZFS home's syncs for the file bucket and the store, no part of fxdax (TASK-397 filed for the bench's default). Results: ubu1 /mnt/fxdax-scratch/t371e, ~/src/fxdax-t350-results/t371c and t371d.
DoD #1: main CI 43200591789 (macOS and Linux) green on 693849713 with TASK-371 in; #2 scripts/upstream-changes.py --check passes on 10f613290.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A durable sync is one bucket round trip: sync(2), syncfs and freeze make one durable commit, not three (the guest's durable guard over the final device flush only; the VFS's wait=0 sync_fs and a freeze's second sync made ordered), and a durable commit is its segment's create alone, no lease read, the writer's create as its fence (fences_kept/takeovers_kept, then TASK-388's one keep rule; TLA+ mutations, simulation profiles, layout FORMAT 4). On ubu1 at 25 ms bucket latency the sync-loop went 3.2/s to 30.1/s (store on ext4 scratch, bucket on tmpfs), its durable commit 32.5 ms against the bucket's ~30 ms round trip. Verified by syncs_make_one_commit_durable, durable_commits_cost_their_segments_create, the TLA+ mutation suite, the simulation needs-tests, main CI 43200591789, and the ubu1 bench.
<!-- SECTION:FINAL_SUMMARY:END -->
