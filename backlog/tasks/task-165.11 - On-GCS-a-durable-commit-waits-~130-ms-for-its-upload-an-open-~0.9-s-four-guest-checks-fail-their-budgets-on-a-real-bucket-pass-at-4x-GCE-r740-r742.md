---
id: TASK-165.11
title: >-
  On GCS a durable commit waits ~130 ms for its upload, an open ~0.9 s: four
  guest checks fail their budgets on a real bucket, pass at 4x (GCE r740, r742)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 09:22'
updated_date: '2026-11-08 00:22'
labels:
  - real-bucket
  - perf
dependencies: []
documentation:
  - docs/measurements/real-bucket-guest-suites-2026-11-06.md
parent_task_id: TASK-165
priority: high
type: bug
ordinal: 645000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every guest suite so far ran on `file://` or `memory://` buckets. Against a real one (GCS, `gs://echophase-fxdax-bench`, us-east4; hosts in us-east1-c, n2-standard-4, jailed, production seccomp filter; main f81174090), four `fxfs-guest-checks*` cases fail on time alone (GCE r740), where the same tree on a file bucket passes them all (r741). Given 4x their budgets (`FXDAX_CHECKS_SCALE=4`, r742), every check passes: nothing hangs, it is slow. `fxfs-guest-docker-dax` passed at 242 s of its 300 s limit (45 s on a file bucket).

**Failed checks, r740 (killed at their budgets, which `host.rs:83-84` doubles for any `FXDAX_EXTENTD_REMOTE`), and as long as they took in r742 / on a file bucket (r741):**

| check | r742 GCS | r741 file | x |
|---|---|---|---|
| torture | 81.6 s (checks), 92.6 s (-dax) | 16.5 s, 17.0 s | 4.9-5.5 |
| enospc (passes at 2x, 56 s of 66) | 55.5 s | 16.4 s | 3.4 |
| unbind_waits_for_unmount | 8.3 s | 0.84 s | 9.8 |
| append_writeback (64 fdatasyncs beside a syncfs loop) | 8.6 s | 1.05 s | 8.2 |
| file_attributes (an unmount and mount) | 3.8 s | 0.21 s | 17.9 |
| cow_store_persists | 2.0 s (r740: 10.8 s, killed) | 0.35 s | 5.9-45 |

Whole cases: checks 175 s against 60 s, checks-dax 209 s against 75 s. Every other suite passed on GCS, 1.2x to 3.2x slower: fxfs-guest, -dax, -crash*, -live-fork, -snapshot*, -suspend-dax, -migrate*-dax, -checks-overrun, extent-guest; pgbench and pgbench-dax 1.01x.

**Where the time goes**, the main daemon's requests (`extentd-stats.txt`), per request, r740 against r741, the same 20 cases:

| request | GCS | file | x |
|---|---|---|---|
| open | 884 ms | 15 ms | 58 |
| fork | 697 ms | 26 ms | 27 |
| fault | 28.2 ms | 0.31 ms | 91 |
| read | 2.20 ms | 0.076 ms | 29 |
| commit | 23.4 ms | 4.5 ms | 5.2 |
| write | 1.35 ms | 0.35 ms | 3.9 |
| commits' wait for the bucket (`commit-wait-ns` / `commits`) | 3.6 ms | 0.56 ms | 6.4 |

- Durable commits: `ExtentStore::commit_and_wait` (crates/fxdax/src/extents.rs:1255) waits in `make_durable` (extents.rs:1172-1177) for `Remote::wait_uploaded`, the segment's PUT and its acknowledgement after a lease read (doc-7, Uploads; doc-14, Commits, closing, uploads). The guest's fsync and syncfs park in `fxfs_rt_park` meanwhile (r740's stacks: `fxfs_fsync`, `fxfs_sync`, `__do_sys_msync` and `syncfs` in state D at every timed-out check). On GCS a guest fdatasync costs about 130 ms (append_writeback: 8.6 s over 64 rounds), against 16 ms on a file bucket.
- Opens and forks: 0.7-0.9 s each on GCS, which every VM boot, snapshot, migration and fork pays.
- Reads and faults of extents only in the bucket (the image a case's store forks): each page-in is a ranged GET (crates/fxdax/src/cache.rs:399-410), with no part cache in the test daemons (the other task filed with this one).

**Bucket requests**, the main daemon, r740, 20 cases: get 3,709, get-range 11,306, list 208, create 7,606, update 15, delete 1; 412 MB of segments, 1.8 MB of own extents and 1.1 MB of checkpoints up, 826 MB down. r742 (4 cases): get 1,545, get-range 6,774, list 48, create 3,510, update 5; 230 MB up, 1.7 GB down. The file run's counts aren't comparable: on a file bucket the cases' stores keep branches the daemon's `Stats` doesn't count (their own, TASK-165.6).

This is what TASK-165.9's recovery and uploader and TASK-165.10's measurements need: what a durable commit, an open and a fork cost on a real bucket, by phase. Every case's time on both buckets, and the runs: docs/measurements/real-bucket-guest-suites-2026-11-06.md.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A durable commit, an open and a fork on a GCS bucket are measured by phase (segment close, PUT, lease read, acknowledgement, checkpoint) and recorded in docs/measurements
- [ ] #2 fxfs-guest-checks and fxfs-guest-checks-dax pass on a GCS bucket within their budgets, the remote scale in host.rs set from that measurement rather than r353's 2.0
- [ ] #3 fxfs-guest-docker-dax passes on a GCS bucket with its limit at least 1.5x what it takes
- [ ] #4 A guest fsync on GCS costs no more round trips to the bucket than doc-7 says a durable commit needs, checked by a test that counts them
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Count and time each bucket round trip on the commit, open and fork paths (Stats::request_ns; a test pinning a durable commit's requests; bucket_round_trips_by_phase, a measurement against a real bucket).
2. Cut the round trips the protocol doesn't need in series: the writer's reads once it holds the lease (record, recovery, segments listing) at once; the lease read alongside the record before recovery, the taking conditional on it; a checkpoint's moves listing alongside its children's record reads.
3. Measure on GCE against gs://echophase-fxdax-bench (one host in us-east4, the bucket's region): the phases before and after the cuts; fxfs-guest-checks, -checks-dax and -docker-dax on GCS at 4x budgets, each check's time.
4. Set the checks' remote scale in host.rs and docker's limit from that measurement; verify on GCS; record in docs/measurements.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Round trips, as the code has them (main 4fe1dd081):
- A durable commit (ExtentStore::commit_and_wait, crates/fxdax/src/extents.rs:1316, make_durable :1238 -> extent_store::upload::Remote::wait_uploaded): the uploader's gather closes and frames the open segment (upload.rs:940), its own extents' PUTs a few at once then the segment's create (extent_remote writer.rs:344, upload), then a read of the lease, which acknowledges it (writer.rs:406, acknowledge, branch::holds_lease). Two round trips in series, three with extents of their own: what doc-7's Uploads and Fencing need (acknowledge only after a read of the lease after the creates). Pinned by durable_commits_cost_their_segments_create_and_a_lease_read (fxdax remote.rs): one create, one get. Nothing to cut without changing the protocol; r740-r742's hosts were in us-east1-c, the bucket in us-east4.
- An open (extent_store upload.rs start, :789; extent_remote writer.rs take, :170): the record; a recovery before the lease (checkpoints listed, the checkpoint, footers' tails, the checkpoints listed again, doc-7's newest-checkpoint rule); the lease read and updated; the record again; the recovery on from there; the segments listed; the fence's create; the lease read again. Some 13 round trips in series. Cut: the record, the recovery on and the segments listing at once once the lease is taken (two fewer), and the lease read alongside the first record read, the taking conditional on it (one fewer).
- A fork (fxdax host_store::fork): the child's record read (child_free), the commit's segment created and acknowledged (create, lease read), the child's record created, then the checkpoint that makes the fork: the moves listed, the children's records read, the checkpoint created. Cut: the moves listing alongside the records' reads (one fewer, every checkpoint).
Locally (file bucket, each request 20 ms late, bucket_round_trips_by_phase): durable commit 61 ms (54 ms of bucket, 2 requests), open 364 -> 302 ms, a fork's first open 396 -> 336 ms. The newest_checkpoint rule's seed search found nothing in 0..20,000 after the cut (the interleavings moved); 33,630 finds it (an acknowledged commit lost, rule off), and holds with every rule on: the search widened to 40,000. SIM_SOAK=5 sim part 48/48; macOS workspace 2110/2110, Shuttle.
<!-- SECTION:NOTES:END -->
