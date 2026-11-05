---
id: TASK-160.23
title: 'fxfs-posix: each operation declares what it touches'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:09'
updated_date: '2026-11-05 10:40'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: medium
type: task
ordinal: 594000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Every method remembers by hand whether to drain unlinks, whether the failed-batch gate applies (only transaction_with checks it, crates/fxfs-posix/src/lib.rs:1732-1737), and whether to borrow metadata space (decided at five sites). Explains TASK-79.21, 26.19.9, 82, 60. Proposal: self.begin(Op::...) returning the guard and options from one table.

At 9f81f6018: dax_map_write and dax_update_size, which the review listed among those bypassing the gate, went with writable DAX (37c331a6c). Bypassing it now: clone_range (lib.rs:1026, new_transaction directly), write (677), write_dirty (702), write_many (726, DAX writeback's path, through the handle's own transactions), allocate (466), truncate (setattr, 1580-1591) and xattr writeback (write_xattrs, 1082).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One entry per operation
- [ ] #2 The TASK-79.21 cases as tests
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. One table, Op::rules: for each operation a VFS asks, what of the queued unlinks it comes after (nothing, those of the inode it names, or all queued before it), whether it fails once a batch failed, where its transactions' metadata space comes from (reserved, borrowed, the page cache's dirty room, none), and whether it holds the named inode's truncate guard.
2. Fxfs::begin(op, named) applies the table: drains, gates, guards, and returns Begun, the only maker of fxfs-posix's transactions (transaction, transaction_with, replace_transaction go), with the options its row says.
3. Every public operation begins its Op; the queue's own batches keep their own transaction (borrowed), outside the table.
4. Gate: every change a VFS asks fails once a batch failed (clone, write, write_many, write_dirty, allocate, truncate, xattr writeback newly), recorded in OPEN_QUESTIONS.md # Decisions.
5. Tests from the table: every op against its row (drain, gate), the TASK-79.21 cases; red before for the newly gated.
6. Mutants: patches against lib.rs re-made where the code moved (fsync/rename-goes-ahead as table edits); all caught as before.
7. Verify: nextest -p fxfs -p fxfs-posix, model hunts (CASES=3000, FSCK, FAILURES), mutants.sh, scripts/test.sh workspace on macOS.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
On branch task-160-23, off origin/land-157 (490faad2c): 68bd0471b (refactor, no change of behaviour), 279a2142f (the gate).

Design: enum Op, one per operation a VFS asks that changes the filesystem or reads what a queued unlink changes (23); Op::rules is the table, a row each: after (Nothing / Named: those of the inode it names / All: every one queued before it, then the named), changes (fails once a batch failed), room (NoTransactions / Reserved / Borrowed / Dirtied: the page cache's dirty room, the handle's), guards (holds the named inode's truncate guard). Fxfs::begin(op, named) drains, gates and guards and returns Begun, which makes fxfs-posix's every transaction with its row's options: transaction, transaction_with and replace_transaction are gone. The queue's own batches stay outside the table (what operations wait for; Room::Borrowed's options). Pure reads (data, extents, symlink targets, xattrs) and evict begin nothing. The follower's fill barrier (Platform::filling) stays where it was: half of its calls are the guest's reading wrapper, outside fxfs-posix.

Gate: every change fails once a batch failed, the seven that bypassed it too (clone_range, write/write_many, write_dirty, allocate, truncate cut or grow, xattr writeback); write_dirty takes its dirty room first, so one refused settles it as one that fails does. OPEN_QUESTIONS.md # Decisions says why. Not gated: lookup, readdir, attr_by_handle, sync, fsync, statfs, set/remove_xattr (in memory), evict, the queue's batches.

ACs: #1, the table (Op::rules). #2, the TASK-79.21 cases as tests of the class: each_operation_comes_after_the_unlinks_its_row_says (every op, an unlink of what it names and one of nothing it names queued, against its row's after: lookups' and exchanges' link counts, readdir, names changing after the unlinks, syncs and statfs; a hand mutant of readdir's and statfs's Op caught), every_change_fails_once_a_batch_failed (red on 68bd0471b for exactly the seven; green on 279a2142f), with the earlier instances kept (lookups_count_links_queued_to_go, exchanges_count_links_queued_to_go, names_change_after_the_unlinks_queued, statfs_applies_what_is_queued, a_failed_batch_keeps_what_the_kernel_evicts).

Structure: the bypasses were the class this removes: a method choosing its own transaction maker skipped the gate. Now no transaction is made but through a Begun, so no change can miss its row; a new Op won't compile without a row, nor without a case in the tests' call.

Runs (macOS): cargo nextest run -p fxfs -p fxfs-posix 821/821. Model hunts: FXDAX_MODEL_CASES=3000 passed (6402 cuts, 11582 of replays, 2335 s); with FXDAX_MODEL_WORKERS=1 passed (6242 cuts, 1272 s); FXDAX_MODEL_FAILURES=3000 passed (1645 requests failed, 838 operations failed, 10453 cuts, 1220 s); FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1 failed after 2015 s on a journal replay refused (journal finishes at 307150, written to 307200 before a barrier), deterministic, the same on origin/land-157: not this change, filed as TASK-182 with its sequence (passes without fsck after every transaction). mutants.sh: 16 of 17 caught, truncate-gives-up-on-its-tail not caught, as before; fsync- and rename-goes-ahead-of-unlinks re-made as edits of their rows, link-stays-in-graveyard re-made for its moved context. FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast: 2070/2070. Not run: GCE (DoD #1's Linux half), scripts/upstream-changes.py --check.
<!-- SECTION:NOTES:END -->
