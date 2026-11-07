---
id: TASK-236
title: >-
  Directory entries say a special file's type, so readdir looks up no file's
  record (format 63)
status: In Progress
assignee: []
created_date: '2026-11-07 20:39'
labels:
  - fxfs
  - format
dependencies: []
ordinal: 673000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
TASK-149.36 made readdir report FIFOs, sockets and devices as what they are by looking up each file's record, because Fxfs's entries said only file, directory or symlink: one tree lookup per file on every listing, in the guest too. The user chose the on-disk format change over the lookup (OPEN_QUESTIONS, 2026-11-07). fxdax migrates no format: every store made before it is unreadable after.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A directory entry of a special file says which (FIFO, character device, block device, socket), set as it is made or linked, kept across rename
- [ ] #2 fxfs-posix readdir reads no file record to type an entry
- [ ] #3 fsck reports an entry whose type differs from its object's
- [ ] #4 The on-disk format is 63, and the model test, fsck tests and FUSE tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
