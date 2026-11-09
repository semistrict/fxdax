---
id: TASK-272
title: >-
  fxfs-posix every_cut_of_a_short_sequence: a cut waits for journal space too
  long (debug_assert_not_too_long, journal.rs) after Snapshot, Mkdir,
  SetDirXattr, Rename
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-09 00:50'
updated_date: '2026-11-09 01:50'
labels:
  - fxfs
dependencies: []
priority: medium
type: bug
ordinal: 699000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
GCE debug round for TASK-269 (run-20261108T233114Z-d2a31963, host workspace suite, debug build): model_tests::every_cut_of_a_short_sequence failed after 426 s, panicking in debug_assert_not_too_long! in the journal's wait for space (crates/fxfs/src/object_store/journal.rs, the reclaim_event listen loop), each shrink step the same. proptest case: seed 9159821385706884731, [Snapshot, Mkdir(0), SetDirXattr { dir: 2, name: 0, len: 4682, byte: 112 }, Rename(12, 2)]. Host-only code, no VMM involved. Possibly kin to TASK-245 (snapshot_volume waiting for journal space under its source's flush lock).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The case above passes every cut, its wait for journal space ending
- [ ] #2 A regression test with that sequence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced on the release round too (run-20261108T233107Z-6613ee40): failed after 390 s, same test.

Also red in main CI 42542801715 (test (workspace), 620 s, at 1c60cffb2), green in 42533641317 at bbed9afd0: intermittent, proptest seeds random per run.

2026-11-09: root cause: TASK-245's fix had snapshot_volume call Journal::check_journal_space directly, skipping the maybe_start_flush_task that reservation_for_transaction does first. A cut's remount that has committed nothing yet (super-block at checkpoint 0, 17036 bytes replayed, reclaim 16 KiB) has no flush task, so no compaction ran and the Snapshot waited forever. Fixed at 527e42a34: FxFilesystem::wait_for_journal_space starts the flush task, then waits; the snapshot and every checked transaction use it. Pinned by every_cut_of_a_snapshot_a_directory_its_xattr_and_a_rename: 60 s hang before, 0.19 s after. fxfs + fxfs-posix 886/886 on macOS, upstream-changes --check passes. TASK-245 itself stays fixed; this was a regression its fix introduced.
<!-- SECTION:NOTES:END -->
