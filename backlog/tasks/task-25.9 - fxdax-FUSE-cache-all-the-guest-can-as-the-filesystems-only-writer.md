---
id: TASK-25.9
title: 'fxdax FUSE: cache all the guest can, as the filesystem''s only writer'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-10 21:46'
updated_date: '2026-10-10 21:46'
labels: []
dependencies: []
parent_task_id: TASK-25
ordinal: 34000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Only the guest changes an fxdax filesystem, so its FUSE caches can stay valid indefinitely, and requests with nothing to do need not be sent. Before: AUTO_INVAL_DATA dropped a file's cached pages when its mtime changed (mmap reads without DAX 124x ext4); OPEN, FLUSH and RELEASE were sent for files that have no handles; missing names were looked up again each time (ENOENT); directories' entries and symlinks were read each time. A create costs 6 requests (GETATTR, LOOKUP, CREATE, WRITE, FLUSH, RELEASE) of ~110-190 µs each under nested KVM.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 INIT offers no AUTO_INVAL_DATA or ATOMIC_O_TRUNC, and offers CACHE_SYMLINKS; OPEN and FLUSH are refused (ENOSYS), so the guest stops sending OPEN, FLUSH and RELEASE; missing names are negative entries with the entry timeout; directories open with FOPEN_KEEP_CACHE and FOPEN_CACHE_DIR; created files with FOPEN_NOFLUSH
- [ ] #2 The fxdax guest tests pass on macOS and under KVM on GCE
- [ ] #3 Creates, and mmap reads without DAX, faster on GCE than gce-fs-bench-2026-10-10T2124Z
- [ ] #4 FUSE_WRITEBACK_CACHE measured on GCE, and enabled if it makes the workloads faster
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
