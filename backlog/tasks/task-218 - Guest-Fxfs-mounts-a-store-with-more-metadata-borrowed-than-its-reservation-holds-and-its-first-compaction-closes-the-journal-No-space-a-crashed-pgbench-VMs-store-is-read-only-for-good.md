---
id: TASK-218
title: >-
  Guest Fxfs mounts a store with more metadata borrowed than its reservation
  holds, and its first compaction closes the journal (No space): a crashed
  pgbench VM's store is read-only for good
status: Done
assignee:
  - '@claude'
created_date: '2026-11-06 19:26'
updated_date: '2026-11-06 20:09'
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
- [x] #1 What borrowed the metadata space is known, with file:line, from the kept store mounted with Fxfs instrumented as TASK-26.6.1 did
- [x] #2 A test that fails first: a store through many crashes and replays under a pgbench-like load mounts and compacts
- [x] #3 A store whose borrowed space exceeds its reservation at mount compacts rather than closing its journal, or the reservation is sized for it
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Mount the kept store read-only, instrumented, from a local copy of its bucket; see what replay adds to the borrowed space.
2. A test that fails on main: a store mounted repeatedly must find the borrowed space as it had it.
3. Fix replay at the root: the journal records what replay needs (format 62).
4. Class oracle in the model: every clean remount compares the borrowed space replayed with the space at unmount.
5. Suites and 1,000-case hunts, plain and fsck, on macOS and Linux (Docker).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
## The kept store, instrumented
Slot 1 (branch a08b156044fc9ddb) was copied with its bucket to a local file bucket and mounted read-only, with prints on every UpdateBorrowed replay applies. The super-block already said 9,748,916 bytes borrowed. Replay of the 82 KB of journal since it added about 340 KB (10,087,020 against required 10,395,468: 308,448 left for compaction, which needed 741,376). Within that window two patterns repeat:
- 9-byte transactions after each sync's padding borrow about 16 KB each, live too: the next transaction's recorded value matches what replay computed. That is make_room's padding transaction falling back to borrowing on a full store (TASK-26.6.1's path), which compaction repays.
- 67-byte Borrowing transactions (unlinks, tombstones) borrow nothing live: the next recorded value is unchanged. Replay counted 268 bytes for each. Only the last one before a mount counts, but whatever replay adds is kept: the next super-block records it, and compaction only repays journal it shrinks.
The 9.7 MB was accumulated before the last super-block, so the store can't show its history; the mechanism was found in the code and by the tests below.

## Cause
Replay doesn't find the borrowed space as the filesystem had it, and each mount keeps what it adds. object_manager.rs replay_mutations (was :399-404) sets borrowed to the last UpdateBorrowed plus four times the journal used from the end of the last transaction it replayed. Live, did_commit_transaction (:546-557) adds four times the usage from the object manager's last_end_offset, less what a Borrowing transaction's reservation pays (569fb5e). They differ in two ways:
1. A Borrowing transaction (TRANSACTION_METADATA_MAX_AMOUNT reserved) pays all its usage, so live it borrows nothing; replay counts all of it, up to 98 KB.
2. A gap before a transaction (a sync's padding, or a torn tail a replay discarded) under TRANSACTION_MAX_JOURNAL_USAGE / 2 is paid live by make_room's padding transaction (journal.rs make_room, TASK-26.6.1). That transaction has no mutations, so only a lone Commit reaches the journal and replay can't see it: it counts the gap as borrowed, up to 4 x 12 KB.
So each mount whose replay ended after such a transaction borrowed up to 100-150 KB more than the filesystem had, and kept it: the amount reserved at mount is required minus borrowed, and compaction only repays the journal's share. A store mounted about a hundred times (37 rounds of several VMs, crashes making the discarded gaps) gets about 10 MB borrowed, more than its largest compaction can spare. Pinned: journal.rs test_a_mount_borrows_what_the_filesystem_had_borrowed, a Borrowing transaction then close and open, 8 rounds; on main each mount added 16,384 ("left: 554712, right: 554516" at round 0).

## Fix (format 62)
Mutation::UpdateBorrowed { borrowed, from, paid }: from is the object manager's last_end_offset, where the live accounting counts the transaction's usage from (padding transactions included); paid is what a Borrowing reservation pays (0 if Borrowed). replay_mutations sets borrowed = borrowed + space - min(space, paid), with space = 4 x (end - from), exactly as did_commit_transaction adds. MutationV62 and JournalRecordV62; LATEST_VERSION 62, so format-61 stores, such as the soak's, are refused rather than read with their inflated borrowed. ObjectManager::borrowed_as_replayed keeps the value replay found, before the mount's own transactions.

## Blind spot and tests (AGENTS.md)
- Why green: test_replay_is_identical compares replay with live only for transactions that borrow everything. Nothing compared the borrowed space across a mount, and the excess grows slowly, one mount at a time, which only a long-lived store under crashes reaches.
- Class oracle: model_tests.rs World::remount (every Op::Remount, and the end of every sequence) compares the borrowed space at unmount with ObjectManager::borrowed_as_replayed. With the old replay rule put back it fails on the one-op sequence [Tmpfile(0)] ("570812 bytes borrowed as replayed, 554596 as unmounted"); the pinned test fails too.
- Looked around: MetadataReservation::Hold and Reservation transactions that exceed their hold borrow the rest with only a warning and no UpdateBorrowed, which replay can't see either. Debug builds panic on it, and it errs the safe way (replay borrows less, so the mount reserves more). Left as is.

## Acceptance
- AC 1: the kept store mounted with Fxfs instrumented shows replay counting what live didn't borrow (Borrowing transactions, paid gaps), with file:line above. Its 9.7 MB is baked into its super-block, so the accumulation itself is shown by the pinned test, not by the store.
- AC 2: test_a_mount_borrows_what_the_filesystem_had_borrowed, eight mounts, fails on main (16,384 more a mount). The model oracle runs at every clean remount of every hunt sequence.
- AC 3: met by sizing. A mount now finds the borrowed space the filesystem had, so the reservation it makes is what the live filesystem kept for compaction, and no mount adds to it. Stores already inflated are format 61, which is refused.

## Verification (on origin/main af049a8aa)
- macOS: cargo nextest run -p fxfs -p fxfs-posix -p storage_device -p fxdax 1,189 of 1,189; whatever_a_vfs_asks FXDAX_MODEL_CASES=1000: passed (3,160 cuts, 4,234 of replays, 546 s); with FXDAX_MODEL_FSCK=1: passed (3,173 cuts, 4,475 of replays, 826 s). The remount oracle ran in both.
- Linux (Docker arm64): fxfs, fxfs-posix and storage_device 902 of 902; hunt passed (3,033 cuts, 4,144 of replays, 486 s); fsck hunt passed (3,056 cuts, 4,210 of replays, 792 s).
- cargo check --workspace, and the tests of the crates that use fxfs, compile; upstream-changes.py --check passes. GCE and the soak not run.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Replay re-derived the metadata space borrowed differently from how the live filesystem borrowed it. It counted a Borrowing transaction's whole usage, which its own reservation had paid, and the padding or discarded tail before a transaction, which an unjournaled padding transaction had paid. Each mount kept what it added and no compaction repaid it, so a store mounted about a hundred times through crashes ended with 10 MB borrowed. Format 62's UpdateBorrowed records where the usage counts from and what the reservation pays, and replay adds exactly what live did. Pinned by test_a_mount_borrows_what_the_filesystem_had_borrowed (red before), with a model oracle at every clean remount. Suites and 1,000-case hunts, plain and fsck, pass on macOS and Docker Linux. GCE and the soak not run.
<!-- SECTION:FINAL_SUMMARY:END -->
