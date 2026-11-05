---
id: TASK-160.20
title: 'Fxfs: one way to tombstone an unlinked object in a transaction'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-03 13:08'
updated_date: '2026-11-05 04:39'
labels:
  - structure
dependencies: []
parent_task_id: TASK-160
priority: high
type: task
ordinal: 591000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Freeing unlinked objects many to a transaction is done twice with different rules: ObjectStore::tombstone_objects (the graveyard's batch) and fxfs-posix's tombstone_now, which reaches into the transaction to take back a graveyard entry and writes the name's raw record. Explains TASK-82, 153.2's queued-unlink cases. Proposal: ObjectStore::txn_tombstone_unlinked() owning the graveyard-entry cases, the empty-object fast path and is_full; Directory::txn_remove_known_child().
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 One tombstoning path for both
- [ ] #2 Model hunts with batched_graveyard and fsck pass; fxfs-posix mutants still fail
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. ObjectStore::txn_tombstone_unlinked(transaction, object_id) -> bool: is_full, the object in the graveyard (the transaction's entry or the tree's) or Inconsistent, keys removed, the unallocated fast path (tombstone_if_unallocated, made private) or trim_some's tombstone, and on success the entry gone: the tree's removed, the transaction's taken back.
2. ObjectStore::tombstone_objects (graveyard batch) and replace_child_with_purge's purge go through it; fxfs-posix's tombstone_now deleted.
3. Directory::txn_remove_known_child(transaction, name, object_id) -> last name: the name's record by key and the object's reference, for directories whose names are keys; apply_unlinks uses it.
4. Tests: txn_tombstone_unlinked's graveyard-entry cases (fxfs), txn_remove_known_child; mutant tombstone-now-leaves-the-entry moved to the new code; mutants.sh --check.
5. Verify: nextest -p fxfs -p fxfs-posix -p storage_device, model hunt FXDAX_MODEL_CASES>=3000, mutants that touch the code, FXDAX_TEST_PARTS=workspace scripts/test.sh, upstream-changes.py --check.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Design (0ff7805e7): ObjectStore::txn_tombstone_unlinked(transaction, object_id) -> bool is the one path. It returns false on a full transaction; fails Inconsistent for an object not in the graveyard (the transaction's pending entry decides, else the tree's); removes keys; takes the unallocated fast path (tombstone_if_unallocated, now private) or trim_some's Tombstone(Object); and on success removes the entry: the tree's by a None mutation (which replaces any pending Some), else the transaction's pending one taken back, so graveyard_count stays true. Callers: ObjectStore::tombstone_objects (graveyard batches, which now get the fast path, and an object not in the graveyard is refused rather than tombstoned live); replace_child_with_purge (upstream's third copy, which left a tree Trim entry for an object gone, the TASK-82 class); fxfs-posix apply_unlinks (tombstone_now deleted). Dropped: tombstone_now's refusal to trim_some an allocated object with a tree entry, since the graveyard's own tombstone_object removes a Trim entry the same way. Directory::txn_remove_known_child(transaction, name, object_id) -> last name: by key, refs -1, no times, InvalidArgs unless names_are_keys() (DirType::Normal); casefold and encrypted directories still go through replace_child. Mutant tombstone-now-leaves-the-entry renamed a-tombstone-leaves-a-trims-entry and moved to txn_tombstone_unlinked.

Tests: test_txn_tombstone_unlinked (tree entry, pending entry, tree Trim plus pending Some, two attributes left in the graveyard, not-in-graveyard refused, graveyard_count, fsck) replaces test_tombstone_if_unallocated; test_txn_remove_known_child. Found on the way, not fixed: upstream's trim_or_tombstone removes a graveyard attribute entry for each non-DATA attribute whether or not one exists, and counts it, so graveyard_count (a u64 inspect metric) underflows. Seen by tombstone_object of an object with a TEST_ID attribute.

Verified on macOS: cargo nextest run -p fxfs -p fxfs-posix -p storage_device, 868 passed, 2 skipped; FXDAX_MODEL_CASES=3000 model hunt (half of the seeds batched_graveyard) passed in 3016 s (6481 power cuts, 11811 replay cuts); mutants.sh --check, 17/17 apply; scripts/upstream-changes.py --check passed. Not done: the full mutants.sh run (2 of 17 caught, a-batch-loses-its-last-run and a-directory-moved-stays-counted, before the disk filled: other worktrees' target/ dirs, 163 MiB free), the FXDAX_MODEL_FSCK hunt, FXDAX_TEST_PARTS=workspace scripts/test.sh, GCE.

Reruns after the disk was freed (2026-11-04): mutants.sh, 16 of 17 caught (a-tombstone-leaves-a-trims-entry by a_device_that_fails_loses_nothing_durable in 38 s); truncate-gives-up-on-its-tail not caught, the same at the base 9f81f6018 (83 s), so not this change's: its catching test is a gap of its own. FXDAX_MODEL_FSCK=1 FXDAX_MODEL_CASES=200 hunt passed (450 power cuts, 796 replay cuts, 223 s). FXDAX_TEST_PARTS=workspace scripts/test.sh --no-fail-fast: 2054 passed, 18 skipped. Not run: GCE (Linux). Note: mutants.sh's patch leaves *.orig files for a patch that applies at an offset.
<!-- SECTION:NOTES:END -->
