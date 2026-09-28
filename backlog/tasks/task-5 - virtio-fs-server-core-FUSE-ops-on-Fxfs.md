---
id: TASK-5
title: 'virtio-fs server: core FUSE ops on Fxfs'
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
labels: []
milestone: m-0
dependencies:
  - TASK-1
  - TASK-2
type: feature
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The server that libkrun calls for guest filesystem requests, backed by FxVolume/FxDirectory/FxFile. Reads and writes go through kernel streams, which block on the pager, so they must not run on the Fxfs executor threads.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 init, lookup, forget, getattr, setattr (size, mode, times), open, create, read, write, flush, fsync, release, fallocate, statfs, readdir(plus), mkdir, unlink, rmdir, rename, symlink, readlink and link work, each shown by a test driving the server through its FUSE interface
- [ ] #2 Blocking stream I/O never runs on an Fxfs executor thread, asserted in debug builds
- [ ] #3 Data written through the server survives unmount and remount and passes fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
