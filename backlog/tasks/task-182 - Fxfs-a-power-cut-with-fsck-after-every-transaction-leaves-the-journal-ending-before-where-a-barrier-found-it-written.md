---
id: TASK-182
title: >-
  Fxfs: a power cut with fsck after every transaction leaves the journal ending
  before where a barrier found it written
status: To Do
assignee: []
created_date: '2026-11-05 10:00'
updated_date: '2026-11-05 10:01'
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
- [ ] #1 The sequence pinned as a model test, failing before the fix
- [ ] #2 Its cause named: the journal, or the fsck hook
- [ ] #3 The fsck hunt (FXDAX_MODEL_CASES=3000 FXDAX_MODEL_FSCK=1) passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
