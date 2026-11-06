---
id: TASK-217
title: >-
  After the daemon was killed and the cache lost, a store's extent its index
  places in the bucket can't be read: a checked database fails to start (EIO)
status: To Do
assignee: []
created_date: '2026-11-06 16:32'
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
