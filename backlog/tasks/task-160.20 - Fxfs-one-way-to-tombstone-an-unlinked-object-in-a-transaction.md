---
id: TASK-160.20
title: 'Fxfs: one way to tombstone an unlinked object in a transaction'
status: To Do
assignee: []
created_date: '2026-11-03 13:08'
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
