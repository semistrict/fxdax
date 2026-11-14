---
id: TASK-315
title: >-
  extent_store's durable_commits_keep_the_open_segments_pages_cached_on_the_host
  fails on every run on ubu1, whose target dir is on ZFS: no file's pages are in
  the page cache
status: To Do
assignee: []
created_date: '2026-11-10 08:58'
updated_date: '2026-11-14 14:03'
labels: []
dependencies: []
priority: medium
type: bug
ordinal: 723000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
On ubu1 (Ubuntu 26.04, kernel 7.0.0-43, home on ZFS rpool/USERDATA) tests/store.rs's durable_commits_keep_the_open_segments_pages_cached_on_the_host fails 3 of 3 at origin/main f36453323: '0 of the open segment's 8 pages cached'. Its tempdir is in CARGO_TARGET_TMPDIR, under the worktree's target on ZFS, whose buffered file data lives in the ARC, not the page cache, so mincore finds none resident whatever the store does; HostDir::is_direct still says the file system has a page cache. ubu1 is where all Linux testing now goes, so every full extent_store run there fails on it (nextest cancels the rest).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The test runs its files on a file system with a page cache (e.g. /dev/shm or a tmpfs TMPDIR), or HostDir/is_direct tells ZFS apart and the test skips nothing silently but asserts what holds there
- [ ] #2 cargo nextest run -p extent_store passes on ubu1 and still fails there with the POSIX_FADV_DONTNEED-on-sync mutant it guards against
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-14 (TASK-406 runs on ubu1): fails at main 123a2cbb4 too, and with TMPDIR on the ext4 scratch disk (/mnt/fxdax-scratch), so the test's temporary directory's filesystem isn't the cause; only the target dir was on ZFS then. Panics at crates/extent_store/tests/store.rs:2241: '0 of the open segment's 8 pages cached'.
<!-- SECTION:NOTES:END -->
