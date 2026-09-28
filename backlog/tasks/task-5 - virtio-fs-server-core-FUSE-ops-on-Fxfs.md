---
id: TASK-5
title: 'virtio-fs server: core FUSE ops on Fxfs'
status: To Do
assignee: []
created_date: '2026-09-28 02:41'
updated_date: '2026-09-28 02:53'
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
- [ ] #1 A synchronous, errno-returning filesystem API over FxVolume/FxDirectory/FxFile provides init, lookup, forget, getattr, setattr (size, mode, times), open, create, read, write, flush, fsync, release, fallocate, statfs, opendir, readdir(plus), releasedir, mkdir, mknod, unlink, rmdir, rename, symlink, readlink and link, each shown by a test of that API
- [ ] #2 A libkrun FsBackend adapter on vendor/libkrun branch fxdax maps FUSE requests onto that API, with FUSE node ids mapped to Fxfs object ids and lookup counts
- [ ] #3 Blocking stream I/O never runs on an Fxfs executor thread, asserted in debug builds
- [ ] #4 Data written through the API survives unmount and remount and passes fsck
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/kernel-cfg.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Tests target the fxdax crate API directly; FUSE-level coverage comes from the libkrun e2e test (TASK-10), as with the RedoxFS backend (doc-1).
<!-- SECTION:NOTES:END -->
