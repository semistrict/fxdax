---
id: TASK-371
title: >-
  A durable sync takes 12 bucket round trips where one would do: sync(2) makes 3
  durable commits, each waits for 2 upload cycles, each a segment PUT then a
  lease GET, in series (3/s at 25 ms latency)
status: To Do
assignee: []
created_date: '2026-11-12 01:53'
labels:
  - bug
  - extentd
  - performance
dependencies: []
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
- [ ] #1 A durable commit with nothing new since the last durable one returns without an upload, or the guest sends none: sync(2) costs one durable commit, not three
- [ ] #2 A durable commit waits for one upload cycle, not two
- [ ] #3 The lease check of an acknowledgement goes beside the segment's PUT, or a lease held by time needs none: one round trip a durable commit
- [ ] #4 bench-fs-fxfs-guest-dax's sync-loop on ubu1 at 25 ms bucket latency: 25/s or more
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (ubu1, under fxjob)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
