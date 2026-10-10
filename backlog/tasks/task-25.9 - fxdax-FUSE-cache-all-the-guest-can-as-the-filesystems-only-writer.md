---
id: TASK-25.9
title: 'fxdax FUSE: cache all the guest can, as the filesystem''s only writer'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-10 21:46'
updated_date: '2026-10-10 22:19'
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
- [x] #1 INIT offers no AUTO_INVAL_DATA or ATOMIC_O_TRUNC, and offers CACHE_SYMLINKS; OPEN and FLUSH are refused (ENOSYS), so the guest stops sending OPEN, FLUSH and RELEASE; missing names are negative entries with the entry timeout; directories open with FOPEN_KEEP_CACHE and FOPEN_CACHE_DIR; created files with FOPEN_NOFLUSH
- [x] #2 The fxdax guest tests pass on macOS and under KVM on GCE
- [x] #3 Creates, and mmap reads without DAX, faster on GCE than gce-fs-bench-2026-10-10T2124Z
- [x] #4 FUSE_WRITEBACK_CACHE measured on GCE, and enabled if it makes the workloads faster
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Step A: no AUTO_INVAL_DATA or ATOMIC_O_TRUNC; CACHE_SYMLINKS; OPEN and FLUSH refused (and so RELEASE not sent); negative entries; FOPEN_KEEP_CACHE|FOPEN_CACHE_DIR on directories; FOPEN_NOFLUSH on created files. 2. Step B: measure FUSE_WRITEBACK_CACHE, enable only if faster. 3. Guest tests on macOS and GCE; benchmark on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Step B (T2151Z): FUSE_WRITEBACK_CACHE measured and not enabled: unlinks 3,530 -> 2,560/s, creates without DAX 2,193 -> 1,973, sequential writes without DAX 58 -> 43 MiB/s; random writes without DAX 4,560 -> 5,469 and the build 2.5% quicker. With it, closing a written file writes it back and sends its times (fs/fuse/file.c fuse_flush).

Step A (070afad, T2146Z): mmap random reads without DAX 9,632 -> 1.20M/s (ext4's), creates 1,545 -> 2,193/s. Full run with TASK-25.10 (T2202Z): creates 2,909/s without DAX, 2,837 with. Validation: fxdax tests (86 passed); the 4 fxdax guest tests on macOS, and on GCE under KVM (scripts/test-gce.sh: 1864 workspace tests and 4 guest tests passed); upstream-changes --check passes.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
fxdax's FUSE server lets the guest cache all it can as the filesystem's only writer: cached pages are kept (no AUTO_INVAL_DATA), missing names are negative entries, directories' entries and symlinks are cached, and OPEN, FLUSH and RELEASE are no longer sent (OPEN and FLUSH refused; truncation by SETATTR). The writeback cache was measured and left off: it cost unlinks, creates and sequential writes more than it saved. On GCE: mmap random reads without DAX at ext4's 1.20 million a second (from 9,632), creates 1,545 -> 2,193 a second (2,909 with TASK-25.10). Verified by the FUSE tests and the guest tests on macOS and under KVM.
<!-- SECTION:FINAL_SUMMARY:END -->
