# A durable commit, an open and a fork on GCS, by phase (TASK-165.11)

On GCS a guest's fdatasync took about 130 ms and a store's open about 0.9 s (GCE r740, r742, hosts
in us-east1, the bucket in us-east4), and four fxfs-guest-checks overran their budgets. Here each
round trip on the commit, open and fork paths is counted and timed against the bucket from a host
in its region, before and after the cuts TASK-165.11 made, and the checks' budgets are set from
what they take there.

**Run.** GCE r980, one n2-standard-4 in us-east4-a (the bucket's region), with storage access;
`gs://echophase-fxdax-bench/gc1-r980/`. The phases: `bucket_round_trips_by_phase` (fxdax
remote.rs, ignored), run by `scripts/test-gce.sh run` with `FXDAX_TEST_PARTS=workspace`,
`FXDAX_TEST_ARGS='--run-ignored only --no-capture bucket_round_trips_by_phase'` and
`FXDAX_PHASES_BUCKET`: a store kept in the bucket, 20 durable commits of a block each, 10 opens (each
with its store's drop, whose clean shutdown writes a checkpoint, two requests), 10 forks, and each
fork's first open. Each operation's time, and the bucket's requests it made by kind, their count
and time (`Stats::request_ns`), are lines in `gcs-round-trips-2026-11-08/`. *Before:* 4a0ef99e2
(main with the measurement, without the cuts). *After:* 06eb04a2d (the cuts, landed in land-c5,
24639d64c).

## What each costs

A request to GCS from its region, in r980 (mean): a GET of a record or lease 23-28 ms, a ranged
GET of an object's tail 19-22 ms, a listing 22-24 ms, a create (a segment, a fence, a record, a
checkpoint) 50-61 ms, a conditional update of a lease 54-75 ms.

| Phase | Before, median | After | Requests (after), in series unless said |
|---|---|---|---|
| durable commit | 88 ms | 78 ms | a create (the segment, 56 ms), then a GET (the lease, 23 ms); local about 2 ms |
| open (and its drop) | 470 ms | 387 ms | 5 GETs, 2 ranged GETs, 5 listings, 2 creates, an update; some at once since the cuts |
| fork | 261 ms | 239 ms | 7.5 GETs, a listing, 3 creates |
| a fork's first open | 469 ms | 385 ms | 5 GETs, 2 ranged GETs, 6 listings, 3 creates |

**A durable commit**, a guest's fsync's: closing and framing the open segment, local, about 2 ms;
the segment's create; a read of the lease after it, which acknowledges it (doc-7, Uploads and
Fencing); local again. Two round trips, as doc-7 needs, pinned by
`durable_commits_cost_their_segments_create_and_a_lease_read`; on GCS about 80 ms in all. r740's
130 ms had the hosts a region away from the bucket. No checkpoint is due in one commit (every 64
segments) and no fork's.

**An open**: the record and the lease, read at once; a recovery before the lease (the checkpoints
listed, the newest read, the footers after it, the checkpoints listed again); the lease's
conditional update, from the read; then the record, the recovery on and the segments' listing, at
once; the fence's create; the lease read again. Before the cuts the lease was read on its own after
the recovery, and the record, the recovery on and the listing went one after another: three round
trips more. The median open falls 83 ms (18%); its bucket time stays as it was (451 ms against
467), now partly at once.

**A fork**: the child's record read; the fork's commit's segment created and acknowledged; the
child's record created; then the checkpoint that makes the fork, its moves' listing and its
children's records' reads at once since the cuts, then its create. 22 ms faster.

## The guest checks on GCS

`fxfs-guest-checks`, `-checks-dax` and `-docker-dax` on GCS (`FXDAX_EXTENTD_REMOTE`), after the
cuts, their budgets at four times the remote scale (`FXDAX_CHECKS_SCALE=4`) so that each check's
time shows: all passed; checks 151.8 s, checks-dax 178.3 s, docker-dax 38.4 s of 300 s. Each
check's time against its budget on a file bucket:

| Check | Suite | Took | Budget on a file bucket | Times |
|---|---|---|---|---|
| append_writeback (64 fdatasyncs beside a syncfs loop) | checks-dax | 10.0 s | 3.0 s | 3.34 |
| torture | checks-dax | 79.1 s | 38 s | 2.08 |
| torture | checks | 67.9 s | 38 s | 1.79 |
| read_only_freezes | checks | 3.1 s | 3.0 s | 1.04 |
| enospc | checks | 33.7 s | 33 s | 1.02 |

The rest took less than their budgets on a file bucket. r353's remote scale, 2, held four of them
short (r740); the remote scale is now 5, 3.34 and half that again (`REMOTE_SCALE`,
test_fxfs_guest_checks/host.rs). docker-dax's 300 s is 7.8 times what it took (r742's 242 s had the
hosts a region away and no part cache in the test daemons).

**At the new scale** (5318933bc, `REMOTE_SCALE` 5, no `FXDAX_CHECKS_SCALE`), r980 again: checks
passed in 140.2 s of their 1,585 s limit, checks-dax in 184.0 s of 2,085 s; the closest,
append_writeback 9.7 s of 15 s, torture 82.3 s of 190 s (checks-dax) and 69.3 s of 190 s (checks).
