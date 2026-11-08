---
id: TASK-149.40
title: >-
  The model test's remaining gaps: snapshots, write_dirty, the guest's platform,
  long symlinks
status: In Progress
assignee:
  - '@claude'
created_date: '2026-11-07 06:02'
updated_date: '2026-11-08 05:52'
labels:
  - testing
  - fxfs
dependencies: []
parent_task_id: TASK-149
priority: medium
ordinal: 663000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.36 surveyed the fxfs-posix model test (`crates/fxfs-posix/src/model_tests.rs`) and covered node metadata; these gaps it found stayed out of its scope. Snapshots and `write_dirty` (the page cache's writeback, paid with dirty room) never run in random sequences. The model runs one `Platform`, the host's (`KEEPS_DIRECTORY_TIMES` false): the guest's, which keeps directories' times itself, so that Fxfs leaves them be (`create_child_file_untimed`, queued unlinks by key), is never checked. Every symlink says the same six bytes: targets kept outside the record (up to `MAX_LONG_SYMLINK`, in `fxdax.symlink.` xattrs, which listing hides) never run, nor do xattrs set on a symlink that has them. What a DAX window maps is checked for which pages, not for the bytes the device holds there.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Random sequences snapshot the volume and check each snapshot against the model as of its operation, across cuts and remounts
- [ ] #2 Random sequences write back with write_dirty, reserving and releasing dirty room as the guest does, on full devices too
- [ ] #3 Some sequences run on a platform keeping directory times, the model holding directories' times as the guest's VFS would leave them to it
- [ ] #4 Symlinks have targets of every length up to the most Fxfs takes, read back across cuts, and their xattrs list only what was set
- [ ] #5 Each new kind of operation is shown to find a bug: a mutant, red
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Symlinks: Op::Symlink with a target of any length up to MAX_LONG_SYMLINK, the model's per node; readlink across cuts; xattrs of symlinks listed as set (the fxdax.symlink. xattrs hidden). Mutant.
2. Platform: World generic over Seeded<KEEPS>, some sequences on the guest's platform, creates leaving their directories' times. Mutant.
3. write_dirty: Op::WriteDirty reserving dirty room (refused when full), written back by write_dirty, which a device filled since doesn't refuse; release_dirty for what's dropped. Mutant.
4. Snapshots: Op::Snapshot of the volume, kept with the model as of it; each checked, opened as a volume, at the end, across remounts and cuts (a cut keeps those durable). Mutant.
5. mutants.sh shows each red.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-08: land-h15 85cf1f982 (CI to follow). AC1-4 built, each kind in the random sequences (op(), short_op()) and pinned: snapshots (Op::Snapshot, View checks each snapshot volume against the model as of it, across cuts and remounts), write_dirty (Op::WriteDirty, reserve then write_dirty or release, no dirty room kept; on a full device of dirtyings halving in size), the guest's platform (World<KEEPS>, a quarter of the sequences, creates leave directories' times), long symlinks (to MAX_LONG_SYMLINK, xattrs listing only what was set). AC5: mutants a-long-symlinks-chunks-are-listed (found by a_device_that_fails_loses_nothing_durable), a-create-times-its-directory-on-the-guest, writeback-pays-for-nothing-with-its-room, a-volume-frees-what-its-snapshot-shares, each killed by mutants.sh. Found: TASK-241 (High), a transaction of the volume between a snapshot's flush and commit frees what the snapshot shares (the batched graveyard's tombstone; remounted, the journal failed with an inconsistency): Op::Snapshot settles the graveyard first meanwhile; a_snapshot_of_a_file_closed_but_not_yet_tombstoned reproduces it, ignored naming TASK-241. Not built: the description's DAX window bytes check, no AC asks it. fxfs-posix 93/93 (1 ignored) on macOS; clippy's disallowed_methods clean.

2026-11-08: land-h15 rebased onto TASK-241's commit: 740a2e71e, CI 42452421895. Hunt: FXDAX_MODEL_CASES=1500 whatever_a_vfs_asks passes (4,038 power cuts, 5,031 of replays, 414 refused), with a_device_that_fails_loses_nothing_durable and every_cut_of_a_short_sequence.
<!-- SECTION:NOTES:END -->
