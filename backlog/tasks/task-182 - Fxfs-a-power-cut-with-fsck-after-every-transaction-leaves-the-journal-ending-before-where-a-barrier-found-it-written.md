---
id: TASK-182
title: >-
  Fxfs: a power cut with fsck after every transaction leaves the journal ending
  before where a barrier found it written
status: Done
assignee: []
created_date: '2026-11-05 10:00'
updated_date: '2026-11-06 00:45'
labels:
  - bug
  - journal
dependencies: []
type: bug
ordinal: 636000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Found by the fsck hunt of TASK-160.23 (FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1, 2026-11-05, macOS), and on origin/land-157 too (490faad2c, before TASK-160.23): not that change. Replay refuses the cut device: "cut 2, during op 22: opening: Journal replay failed: journal replay cut short; journal finishes at 307150, but was written to 307200 before a barrier" (journal.rs, the TASK-153.13 check). Deterministic; passes without fsck after every transaction, so either a real bug that only the extra fsck flushes or barriers reach, or the fsck hook (Setting::with_fsck_each) moving the recorded journal end itself.

Reproduce: in crates/fxfs-posix/src/model_tests.rs, Setting::awkward(12436301060589371142, ROOMY).with_graveyard_of_seed().with_fsck_each(true).holds(ops), with ops: SetDirXattr { dir: 0, name: 0, len: 1, byte: 0 }, Create(1), Tmpfile(0), Tmpfile(0), Symlink(0), Write { file: 10, at: 8192, len: 1, byte: 0 }, Sync, Write { file: 4, at: 40960, len: 4096, byte: 0 }, Sync, Fill { file: 3, page: 0, pages: 3945, stride: 1, byte: 0 }, UnlinkLater(1), Symlink(2), RemoveXattr { file: 0, name: 0 }, Truncate { file: 1, size: 4096 }, Fill { file: 14, page: 3, pages: 3377, stride: 1, byte: 0 }, Write { file: 2, at: 16384, len: 8193, byte: 0 }, Write { file: 8, at: 1, len: 4096, byte: 0 }, Write { file: 1, at: 1, len: 4096, byte: 0 }, Crash, Truncate { file: 8, size: 4294963200 }, Close(5), Symlink(1), Fill { file: 0, page: 0, pages: 2997, stride: 2, byte: 0 }. Fails in 3 s. Touches crates/fxfs/src/object_store/journal.rs, which TASK-160.25 restructures.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The sequence pinned as a model test, failing before the fix
- [x] #2 Its cause named: the journal, or the fsck hook
- [x] #3 The fsck hunt (FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1) passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-05 triage: fixed on main by 61acf7e40 (TASK-160.25's branch; its notes, '## TASK-182'): the journal, not the fsck hook. A sync's barrier follows a flush of whole blocks, whose last may begin a record going on in a block the cut lost; replay stopped at that record's start (307150), short of the end the barrier recorded (307200), and refused. Replay now compares Root::journal_end with the end of the last block it read, or where it discarded a transaction (crates/fxfs/src/object_store/journal.rs:1002). Pinned: crates/fxfs-posix/src/model_tests.rs:2785, a_cut_with_fsck_after_every_transaction_replays_as_far_as_its_barrier, red before and green after (160.25's notes); FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1 34 of 34 (6,827 s, 160.25's notes). Re-run here on main dde8d58d3, macOS: the pinned test passes (18.5 s).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Fixed by 61acf7e40: replay measures the journal's end a barrier recorded against the last block it read, not where its last whole record ended; the pinned model test a_cut_with_fsck_after_every_transaction_replays_as_far_as_its_barrier was red before and is green (re-run on dde8d58d3, macOS), and the fsck hunt at 3,000 cases passed 34 of 34 (TASK-160.25's notes). GCE not run.
<!-- SECTION:FINAL_SUMMARY:END -->
