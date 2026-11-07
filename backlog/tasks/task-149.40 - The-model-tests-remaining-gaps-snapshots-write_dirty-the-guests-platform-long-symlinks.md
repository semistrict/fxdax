---
id: TASK-149.40
title: >-
  The model test's remaining gaps: snapshots, write_dirty, the guest's platform,
  long symlinks
status: To Do
assignee: []
created_date: '2026-11-07 06:02'
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
