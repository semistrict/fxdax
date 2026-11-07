---
id: TASK-217
title: >-
  After the daemon was killed and the cache lost, a store's extent its index
  places in the bucket can't be read: a checked database fails to start (EIO)
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-06 16:32'
updated_date: '2026-11-07 08:45'
labels:
  - bug
dependencies: []
priority: high
ordinal: 651000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67), GCE r757 round 40 of 55, file bucket, jailed (run FXDAX_SEED=15816163861251038920, round seed 13707783981843791170; plan: slot 1 pgbench, its VM killed at 29 s and booted again, the daemon killed at 179 s, then stopped, then the cache lost; fxdax-gc collecting every 15 s, grace 10 s, compacting throughout). 39 rounds before it passed, three of them losing the host. The checking daemon (no faults) opened slot 1's store (branch ec1f64fbe70bbd21, a fork of the base made in round 36) and its VM's PostgreSQL failed to start, the postmaster dying in recovery: the guest copying a DAX page on write got -5 (`fxfs: making 2 pages at 45e4000 of the window private: -5`), as the daemon could not read the extent (`ERROR fxdax::cache: fxdax: reading ExtentId { branch: 17014428950304898337, sequence: 82463372084929 } at 0x0: Errno(EIO)`; sequence 0x4b00000006c1). EIO, not ENOENT, means the store holds the extent and its data is elsewhere (`extent_store::RemoteData`, crates/fxdax/src/cache.rs:399-410, the fetch through the store's remote); the cache being lost, the fetch went to the bucket, and failed. Why is not known: the read's error is mapped to EIO and dropped (cache.rs:657-659), and logged as the errno alone (cache.rs:609-613). Candidates, none shown: the collector compacted or deleted the segment the store's index places the extent in, the index not following the move across the daemon's kill (TASK-175's moves are taken by writers from their checkpoint); or the acknowledgement that took the extent's local file into the cache (Record::Acknowledged, TASK-165.8) recorded a place the segment didn't land at before the kill. Evidence kept: gs://echophase-fxdax-bench/soak67-evidence/r757-round40-state.tar.gz (1.82 GiB: the whole file bucket, every store directory, the soak's ledger, as the round left them), the round's logs in TASK-67's fetched results (round-40: extentd-1..3.log, gc.log, every console). For after TASK-165.9 (extent_store, extent_remote and remote.rs are being rewritten).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The extent is located in the kept state, and why its read fails is known: a missing object, a stale place, or another cause, with file:line
- [ ] #2 A test of the class that fails first: a store reading what it acknowledged after its daemon is killed, its cache lost and the collector compacted, every acknowledged extent readable
- [ ] #3 fxfs-guest-soak replays round 40 green (FXDAX_SEED=15816163861251038920, 40 rounds)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Re-check against TASK-165.9: probe the kept state with the last tree that reads it (ce8ab6a71, local format 7, with the read-failure logging) for why the read fails and where the bucket has the extent.
2. A test of the sequence on main: segment compacted, writer checkpointing the move, segment deleted, host dead with its cache, store opened from its disk.
3. Close with the evidence if the new design passes it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Root cause, from the kept state (gs://echophase-fxdax-bench/soak67-evidence/r757-round40-state.tar.gz, probed at ce8ab6a71 in a scratch worktree, not committed): the store's local index placed extent 0x4b00000006c1 of branch ec1f64fbe70bbd21 in segments/ec1f64fbe70bbd21/0000000000002181 at 30867456 (24576 bytes); the fetch said lost (the logging of ce8ab6a71 named it). That segment was in the branch (0x2186's footer follows 0x2185, epoch 75) and is gone from the bucket; the branch recovered from the bucket (newest checkpoint 0x223f, the checkpointer's epoch 78) has the extent live at moves/moves/segments/ec1f64fbe70bbd21/0000000000002181. So the collector compacted the segment, the lease holder's checkpoint named the moves, and the collector took the segment; the store, killed and opened again by the checking daemon, opened from its index, which never followed moves (TASK-195's class), and read a deleted object.

On main (TASK-165.9, c9207e5d4): no index; opening recovers the branch from the bucket, naming extents as its newest checkpoint does; and an open store follows its writer's moves (TASK-195, Writer::following, Store::moved). Test: remote.rs extents_compacted_read_once_their_host_died_and_its_cache_went (one segment compacted, the writer checkpointing the move, the segment taken by the collector; the host's disk copied as the death left it, its cache not, its bucket cut off; the store opened from the copy reads the extent): green on main, macOS fxdax and extent_store 466/466. It can't be run red on main, whose design has no index to go stale; red is the old design's, shown above from the soak's own state.

Blind spot: no test opened a store after the collector moved what it held; TASK-195's tests the store open all along. This one covers opening after a crash with the cache lost. Structure: the class is gone with the index: one place names where an extent is, the branch's state as the bucket and its writer have it.
<!-- SECTION:NOTES:END -->
