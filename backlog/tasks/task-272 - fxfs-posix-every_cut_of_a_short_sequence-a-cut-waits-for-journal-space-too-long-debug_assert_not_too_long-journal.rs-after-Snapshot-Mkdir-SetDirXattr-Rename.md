---
id: TASK-272
title: >-
  fxfs-posix every_cut_of_a_short_sequence: a cut waits for journal space too
  long (debug_assert_not_too_long, journal.rs) after Snapshot, Mkdir,
  SetDirXattr, Rename
status: Done
assignee:
  - '@claude'
created_date: '2026-11-09 00:50'
updated_date: '2026-11-09 02:26'
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
- [x] #1 The case above passes every cut, its wait for journal space ending
- [x] #2 A regression test with that sequence
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Reproduce with the recorded seed as a pinned every-cut test. 2. Find why the wait for journal room never ends. 3. Fix the cause. 4. Verify the pinned test, the fxfs and fxfs-posix suites and main CI.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Reproduced on the release round too (run-20261108T233107Z-6613ee40): failed after 390 s, same test.

Also red in main CI 42542801715 (test (workspace), 620 s, at 1c60cffb2), green in 42533641317 at bbed9afd0: intermittent, proptest seeds random per run.

2026-11-09: root cause: TASK-245's fix had snapshot_volume call Journal::check_journal_space directly, skipping the maybe_start_flush_task that reservation_for_transaction does first. A cut's remount that has committed nothing yet (super-block at checkpoint 0, 17036 bytes replayed, reclaim 16 KiB) has no flush task, so no compaction ran and the Snapshot waited forever. Fixed at 527e42a34: FxFilesystem::wait_for_journal_space starts the flush task, then waits; the snapshot and every checked transaction use it. Pinned by every_cut_of_a_snapshot_a_directory_its_xattr_and_a_rename: 60 s hang before, 0.19 s after. fxfs + fxfs-posix 886/886 on macOS, upstream-changes --check passes. TASK-245 itself stays fixed; this was a regression its fix introduced.

Verified 2026-11-09: main CI 42549101699 green at 36dbbe2e9 (all jobs: macOS and Linux workspace, clippy scripts, shuttle, sim, guest). The mutant a-snapshot-holds-no-commits-back (re-anchored by 36dbbe2e9 after 527e42a34 changed its context) is still killed by a_snapshot_of_a_file_closed_but_not_yet_tombstoned in 28 s (mutants.sh, 96 cases) with the fix in place. Linux covered by CI's test (workspace) rather than a GCE round.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
snapshot_volume waited for journal room without starting the flush task (a regression from TASK-245's fix), so on a mount that had committed nothing no compaction ran and it waited forever. FxFilesystem::wait_for_journal_space now starts the flush task and waits, for snapshots and transactions alike (527e42a34). Pinned by every_cut_of_a_snapshot_a_directory_its_xattr_and_a_rename: a 60 s hang before, 0.19 s after; main CI 42549101699 green.
<!-- SECTION:FINAL_SUMMARY:END -->
