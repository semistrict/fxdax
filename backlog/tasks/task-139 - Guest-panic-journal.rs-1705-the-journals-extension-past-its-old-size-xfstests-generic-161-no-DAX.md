---
id: TASK-139
title: >-
  Guest panic: journal.rs:1705, the journal's extension past its old size
  (xfstests generic/161, no DAX)
status: Done
assignee:
  - '@claude'
created_date: '2026-10-31 16:32'
updated_date: '2026-11-06 06:56'
labels:
  - bug
dependencies: []
priority: high
ordinal: 485000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE r342, xfstests-fxfs-guest, generic/161, in writeback: 'fxfs: panicked at crates/fxfs/src/object_store/journal.rs:1705:13: assertion failed: inner.writer.journal_file_checkpoint().file_offset < size'. A guest's kernel panics: TASK-60's rule, never panic on what a workload does. The transaction extending the journal has to reach the journal within its old size, and a large reflink test's did not. Logs: /tmp/fxdax-r342-xfstests/e2e/xfstests-fxfs-guest.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 generic/161 passes, without and with DAX
- [x] #2 A workload can't panic the guest here
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Fixes already on main (d7a6d46a4, e9482bc21, b963ed64a): verify generic/161 with and without DAX on GCE.
2. The extension path's asserts no longer panic: the journal stops with an error (with TASK-60).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: the cause: pre_commit kept a chunk's room, 128 KiB, ahead of the journal, whatever the transaction it was about to write: a larger one's records ran past the file's end, and the next commit found itself there, the assertion's panic. It now keeps room for the transaction's records (room_for: their real size once a transaction may take more than a chunk), and extends by that. test_transactions_larger_than_a_chunk_stay_within_the_journal, 3 transactions of 200 mutations of 1500 bytes, replayed from a snapshot with fsck: fails without the fix, 'the journal at 314787, past its size, 262144', the panic itself. All 50 journal tests pass.

2026-11-03 (verification): the fixes on main (d7a6d46a4, e9482bc21, b963ed64a) hold: GCE 2026-11-03 at 33f235911 and again at 155c06bb0, xfstests generic/161 (and 164, 165) passed on xfstests-fxfs-guest and -dax.
Structure: the journal's own transaction (make_room) is sized by constants (MAX_EXTENDED_PER_COMMIT, MAX_ZEROED_PER_COMMIT, CHUNK_SIZE > TRANSACTION_MAX_JOURNAL_USAGE) and pre_commit keeps room_for(transaction) + 2 chunks; with the default chunk that holds. Only a smaller JournalOptions::chunk_size (tests alone) breaks it. Not restructured: the asserts that caught it no longer panic (11c1c9731, TASK-60): the journal stops, the commit fails (EIO in a guest), and replay ends before the transaction. test_own_transaction_past_the_journal_stops_it, a chunk of a block under a zeroing slice, panicked at the 'past its size' assert before, and passes now, its image passing fsck.

Decision moved here from OPEN_QUESTIONS.md on 2026-11-06, as written there; the file keeps a line on it, pointing here, until the user confirms or overturns it.

**The journal's own transaction past its file stops the journal, not the kernel (TASK-60, TASK-139)**

`make_room`'s asserts, that the journal's transaction extending its file fits within the old size and a chunk, now stop the journal (`Inner::terminate`) and fail the commit: the guest gets EIO and stays mounted, and replay ends before that transaction. With the default chunk the sizing holds, so the transaction wasn't rebuilt to fit the room it has.
**Instead:** size the own transaction to the room left, which would keep the filesystem writable even then.
**Undoing it:** restore the two `assert!`s in `Journal::make_room`.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
generic/161's panic (journal.rs's extension past the old size) was fixed on main by d7a6d46a4, e9482bc21 and b963ed64a. Verified on GCE 2026-11-03: generic/161 passes without and with DAX, at main and at this branch. The extension path's asserts now stop the journal with an error instead of panicking (11c1c9731, with test_own_transaction_past_the_journal_stops_it, red before). scripts/test.sh passes on macOS; the Linux workspace's 2170 tests pass on GCE (Linux runs there, not Lima, per AGENTS.md); upstream-changes.py --check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
