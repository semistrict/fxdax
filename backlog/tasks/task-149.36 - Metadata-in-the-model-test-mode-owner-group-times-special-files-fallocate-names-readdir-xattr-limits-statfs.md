---
id: TASK-149.36
title: >-
  Metadata in the model test: mode, owner, group, times, special files,
  fallocate, names, readdir, xattr limits, statfs
status: To Do
assignee: []
created_date: '2026-11-07 05:20'
labels:
  - testing
  - fxfs
dependencies: []
parent_task_id: TASK-149
priority: high
type: task
ordinal: 659000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-222's FUSE mount found fxfs-posix's setattr zeroing a node's mode when one of mode/owner/group alone changed on a node without POSIX attributes (a new volume's root): a bug in basic POSIX metadata no randomized test could find, because the fxfs-posix model test (model_tests.rs) only checks size, links, xattrs and bytes. Its gaps, surveyed 2026-11-07: mode/uid/gid never changed or checked; atime/mtime/ctime never checked (setattr only truncates, plus a forced mtime in forget); every file made S_IFREG, no FIFO, socket or device, rdev unchecked; fallocate (Fxfs::allocate) never called; names are single letters: no 255-byte names, multibyte UTF-8, casefolding directories (which Fxfs has), or invalid names; readdir only read whole from Index(0), never resumed with After(name), with names removed between; xattr values at most 8 KiB and 3 names: no empty values, none near 64 KiB, no many; the root directory, the one node Fxfs makes without POSIX attributes, seldom an operation's target; statfs never held to the model; snapshots and write_dirty never in random sequences. The model must hold each node's attributes as a VFS on Linux would see them, starting from the defaults Fxfs reports for a node without POSIX attributes.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Sequences change random subsets of mode, uid, gid, atime and mtime on any node, the root and directories included, and every check holds each node's mode, uid, gid and rdev to the model, across remounts and power cuts
- [ ] #2 Times are checked as Linux keeps them where Fxfs promises it: a set time reads back; writes, truncations and changes of a directory's names move mtime and ctime no earlier than before
- [ ] #3 Sequences make FIFOs, sockets and character and block devices with rdev, which read back as made, and fallocate files, the model holding the size and zeroed bytes
- [ ] #4 Names include 255-byte and multibyte UTF-8 names, some directories casefold, and readdir is also read resumed after a name, with names removed and added between its reads, each name listed once
- [ ] #5 Extended attributes include empty values, values near the largest Fxfs takes, and more names
- [ ] #6 statfs is held to what the model's files and xattrs must at least take
- [ ] #7 Each new kind of operation is shown to find a bug: a mutant of the code it reaches, red, recorded in the notes; the fxfs-posix suite passes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
