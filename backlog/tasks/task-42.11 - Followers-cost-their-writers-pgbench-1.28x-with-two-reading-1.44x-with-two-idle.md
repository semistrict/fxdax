---
id: TASK-42.11
title: >-
  Followers cost their writer's pgbench: 1.28x with two reading, 1.44x with two
  idle
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-21 09:06'
updated_date: '2026-10-22 09:11'
labels:
  - followers
  - perf
dependencies: []
parent_task_id: TASK-42
priority: high
ordinal: 164000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
docs/measurements/gce-fs-bench-followed-2026-10-19T0828Z (GCE, clean builds, before the fill barrier): a writer's pgbench-tps on fxfs-guest-dax fell from 1,610 alone to 1,259 with two followers reading (pinned to a core of their own) and 1,117 with two following only, idle (1.44x); the idle case, profiled (gce-fs-bench-followed-perf-2026-10-19T0841Z), leaves the host some 70% idle: the writer waits on something, not for CPU. Followers kept up to 432.5 MiB of the writer's frees and lagged up to 881 commits. doc-8 testing 7 and TASK-42 AC3 require no difference beyond noise. Since then main changed the store's commit path (TASK-41: kept frees indexed, 49d1c1c; segments framed off the commit, bab5ac8), which may have moved it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The cost is measured again on the current tree (pgbench, 3 iterations, alone / followed / followed-idle), and its mechanism identified with evidence
- [ ] #2 A writer's pgbench-tps and commit latency with followers attached (reading and idle) are within noise of the writer alone, on GCE
- [ ] #3 Measurements kept in docs/measurements
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Measured again on main 0a3015b (GCE c3-standard-8 us-central1-a, 2026-10-21; docs/measurements/gce-fs-bench-followed-main-2026-10-21T1016Z): pgbench-tps 1,649 alone, 1,293 with two followers reading (1.28x), 1,155 with two idle (1.43x): unchanged by TASK-41's store work. Each case alone in its run, so the daemon's stats are its own (gce-fs-bench-4211-*-2026-10-21T1029Z, -T1031Z), alone vs two idle followers: the writer's WRITE requests 111 us vs 741 us each; a commit's write of the open extents 30 us vs 137 us, its store commit 10 vs 32 us; segments closed and framed 119 vs 1,121 (1.1 MiB vs 108 KiB each), object-store creates 125 vs 1,144, checkpoints uploaded 16 KiB vs 377 KiB, for about the same bytes uploaded; the followers' READs 171,654 (716 MB read from the store's files, 475 us each), ADVANCE/LET_GO 33,159 each. No commit stalled at the lag bound (no 'commits wait for object storage' warning), and durable COMMITs were fewer (103 vs 152). Open: what makes the uploader gather (Store::close_now) ten times as often with followers: its only callers are wait_uploaded/uploaded/wait_lag's asks and its interval; next, count gathers and segment closes by cause.

2026-10-21/22 diagnosis (GCE c3-standard-8, pgbench, counters in a scratch tree): (1) Not the pinning: the followed case with no followers started (the writer pinned to 3 of 4 cores) ran 1,636 tps, as alone. (2) The uploader (remote.rs) re-gathers at once after each gather that found segments: 1,000 gathers with idle followers (917 re-gathers) against 200 alone; each Store::close_now holds the committing lock and the state while it frames and syncs; filed as TASK-41.1. Without the re-gather, segments 146 as alone, but tps only 1,206 -> 1,278 (alone 1,632). (3) Followers' READs (~180k, ~430 us each) bypass the extent cache and read the store's O_DIRECT files; reading them through the cache instead (as followers' maps do) made it worse, 1,041 tps (page-ins read whole parts), and was reverted. (4) Per commit, 'waiting for object storage' is 365 us with followers against 44 alone, with no lag stall logged: wait_lag's ask takes the store's state lock twice (epoch, unreleased_bytes) on every ordered commit, so the writer's commits wait on that lock. Next: time wait_lag's parts and the state lock's holders with followers.
<!-- SECTION:NOTES:END -->
