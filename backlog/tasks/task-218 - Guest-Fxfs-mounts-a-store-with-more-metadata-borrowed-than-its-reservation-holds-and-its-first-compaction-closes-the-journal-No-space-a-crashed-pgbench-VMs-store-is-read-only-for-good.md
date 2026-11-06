---
id: TASK-218
title: >-
  Guest Fxfs mounts a store with more metadata borrowed than its reservation
  holds, and its first compaction closes the journal (No space): a crashed
  pgbench VM's store is read-only for good
status: To Do
assignee: []
created_date: '2026-11-06 19:26'
updated_date: '2026-11-06 19:26'
labels:
  - bug
dependencies: []
priority: high
ordinal: 654000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by fxfs-guest-soak (TASK-67) on GCS, GCE r759 round 38 (run FXDAX_SEED=427836150578482344, round seed 15107218835171068160; no daemon faults: slot 1's pgbench VM killed at 18 s, as doc-12 says keeps every fsync). Slot 1's store was forked from the base in round 1 and never again: 37 rounds of writers, fsx and pgbench, its VMs killed mid-write in many of them, the daemon killed, its host lost twice, all checked whole. The VM booted again on it mounted (replay complete), and 1.8 s later its first compaction failed: `Failed to flush object store 9 ... ObjectStore::flush: allocation failed: Failed to allocate 741376 bytes for owner 4 from reservation with 308716 bytes available (borrowed metadata space: 10086752, max store reservation: 8773632): No space: Journal flush error`; the journal closed, every change failing EIO (PostgreSQL could not start, the checking VM's unmount EIO). The borrowed metadata space exceeded the most the store reserves for it, and compaction, which returns it, could not take what it needs. The same class as TASK-26.6.1 (sync padding borrowed, fixed in 1c6b2cf), here after a long life of crashes and replays: the borrowed space is restored at replay from the last UpdateBorrowed plus the journal since (crates/fxfs/src/object_store/object_manager.rs:399-404), and each Borrowed or Borrowing transaction adds what its reservation doesn't pay (:546-557); what added the 10 MB, and why compactions since the last mount didn't return it, is not known. Evidence: the store directories and the soak's ledger as the round left them, gs://echophase-fxdax-bench/soak67-evidence/r759-round38-state.tar.gz, their branches in gs://echophase-fxdax-bench/soak67-gcs/run-20261106T162704Z-afc14ccc/soak-d57b273ab94a8f6b (kept for this; slot 1 is branch a08b156044fc9ddb); the round's consoles in TASK-67's r759 results (s1-b1, check-s1).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 What borrowed the metadata space is known, with file:line, from the kept store mounted with Fxfs instrumented as TASK-26.6.1 did
- [ ] #2 A test that fails first: a store through many crashes and replays under a pgbench-like load mounts and compacts
- [ ] #3 A store whose borrowed space exceeds its reservation at mount compacts rather than closing its journal, or the reservation is sized for it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
