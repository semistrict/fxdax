---
id: TASK-153.2
title: A transaction says how much room it has left
status: Done
assignee:
  - '@claude'
created_date: '2026-11-01 22:52'
updated_date: '2026-11-03 02:11'
labels: []
dependencies: []
parent_task_id: TASK-153
priority: high
type: task
ordinal: 546000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Nothing owns how much a transaction can hold: every loop guesses with its own constant (MAX_ZEROED_PER_COMMIT, MAX_CLONED_*, WRITE_MANY_BYTES, BATCH_MUTATIONS). Explains TASK-139, the clone fragment bugs, generic/303 and the punch that spun. trim_some nearly has it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Every such loop adds until the transaction says stop, its constant gone
- [x] #2 The model test passes its hunts
- [x] #3 generic/303 and the punch bug, put back, fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Transaction counts its records' journal bytes and says room(); journal asks it. 2. Measure clones and punches at the refactor commit. 3. Convert loops one at a time: trim_some, graveyard batch, allocate, preallocate_range, punch, clone, write_many. 4. Model oracle over every commit, borrowed or reserved; mutants for overfilling. 5. Mutants and hunts.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
What owns the room: Transaction::journal_bytes() counts each record as the journal writes it (mutation_record_bytes, checksums_record_bytes), through add, replace, remove and take; room() is TRANSACTION_ROOM (24,576 less 2,048) less that; is_full() is room() == 0. TRANSACTION_MAX_JOURNAL_USAGE is what really bounds a transaction (four times it is reserved, in did_commit_transaction); TRANSACTION_MUTATION_THRESHOLD only stood for it, at 200 mutations of about 19 to 58 bytes each.

Blind spot: nothing checked a borrowed transaction's size: the reservation's debug panic sees only those that reserve, so a trim could overfill with every test green. The model now checks every commit (Setting::builder's pre-commit hook). Structure: one owner of the size, which is the fix; the constants were each a guess at it.

Kept, with why: BATCH_OBJECTS bounds locks; DELAYED_BATCH bounds locks and a wake threshold; WRITE_ATTR_BATCH_SIZE is an upstream parameter bounding a device write's buffer, 128 blocks of a new attribute, nowhere near the room; journal.rs's MAX_ZEROED_PER_COMMIT and MAX_EXTENDED_PER_COMMIT bound the journal's own transaction against the chunk left ahead, another limit, which borrows and doesn't pass commit_transaction (dead-ends/oversized-journal-room-for-zeroing.md); 128 stays as BLOCKS_PER_TRANSACTION in dirty_reservation.rs, what dirty data pays per block, which the fit test holds.

Step 3 found: a transaction that holds more changed which paths random sequences reach. truncate-gives-up-on-its-tail was caught only by one pinned seed, which write_many's batches moved off a full device; 4,000 cases with the bug back don't find it either. a-queued-evict-leaves-the-object fell to chance, as a batch's file must now have over 1,000 extents. Each now has a deterministic test that went red with its bug back. test_trim's expect_tombstoned used open_object, whose point lookup of the size doesn't see a tombstone in a later layer once a flush moved the record: it asks for the object's record now. Other pinned seeds may have lost their sequence the same way: only the mutants say which still bite.

Validation at 27e3e7c4a (production code as hunted, 07557ca73): Linux workspace and macOS as in the final summary; hunts of 4,000 cases each, plain, workers, failing devices, fsck after every transaction (four runs of 1,000), every cut (two of 2,000), no failure, TASK-150.19 not seen; 17 mutants caught, twice. Measured by records at the refactor commit and at head: clone of 2048 fragments 82 to 20 commits, 225,046 to 220,892 bytes; over 2048 more 84 to 20, 290,495 to 286,231; punch of 2048 11 to 3, 67,344 to 66,768; clone across 2^50 bytes 1 and 253, punch across them 1 and 0, unchanged. Definition of Done #1 left unchecked: Lima wasn't run, Docker stood for it.

Last Linux workspace at 27e3e7c4a (Docker): 2,111 of 2,115. Failed: tenant_sockets_work_bind_mounted_into_jails (Docker only), whatever_a_vm_process_says_harms_no_other (served > 1000, the coverage floor, about 1 run in 3 on any tree, fixed on fix/refusal-gate, a64fb76a9), guests_forks_failing_at_each_step_harm_nothing (load flake on base, as the user says), durable_commits_keep_the_open_segments_pages_cached_on_the_host (extent_store, which doesn't depend on fxfs; seen once, passed in reruns alone). The run before, at 07557ca73, failed only the first; only model_tests.rs changed since. macOS at e7aebe153: 1,963 of 1,963; fxfs and fxfs-posix at 27e3e7c4a: 769 of 769.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
A transaction now counts its records' journal bytes and says how much room it has left; trim_some, the graveyard's batch, allocate, preallocate_range, punch, clone and write_many add until it says stop, their constants gone. Verified by the model's new check of every commit, mutants of generic/303, the punch and overfilling red, hunts of 4,000 cases per setting.
<!-- SECTION:FINAL_SUMMARY:END -->
