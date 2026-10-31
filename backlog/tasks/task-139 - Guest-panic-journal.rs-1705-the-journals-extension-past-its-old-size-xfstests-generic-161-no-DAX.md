---
id: TASK-139
title: >-
  Guest panic: journal.rs:1705, the journal's extension past its old size
  (xfstests generic/161, no DAX)
status: To Do
assignee: []
created_date: '2026-10-31 16:32'
updated_date: '2026-10-31 16:43'
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
- [ ] #1 generic/161 passes, without and with DAX
- [x] #2 A workload can't panic the guest here
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-10-31: the cause: pre_commit kept a chunk's room, 128 KiB, ahead of the journal, whatever the transaction it was about to write: a larger one's records ran past the file's end, and the next commit found itself there, the assertion's panic. It now keeps room for the transaction's records (room_for: their real size once a transaction may take more than a chunk), and extends by that. test_transactions_larger_than_a_chunk_stay_within_the_journal, 3 transactions of 200 mutations of 1500 bytes, replayed from a snapshot with fsck: fails without the fix, 'the journal at 314787, past its size, 262144', the panic itself. All 50 journal tests pass.
<!-- SECTION:NOTES:END -->
