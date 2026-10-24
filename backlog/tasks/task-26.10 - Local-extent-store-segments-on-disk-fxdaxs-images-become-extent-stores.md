---
id: TASK-26.10
title: 'Local extent store: segments on disk; fxdax''s images become extent stores'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-13 22:51'
updated_date: '2026-10-24 00:53'
labels: []
milestone: m-3
dependencies:
  - TASK-26.3
parent_task_id: TASK-26
ordinal: 46000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
With Fxfs storing extents only (TASK-26.3), fxdax's image files must become an extent store on local disk, and the host's device (TASK-26.4) needs the same store. doc-4's host storage: sealed extents under 4 MiB packed in commit order into segment files of about 16-64 MiB, extents of 4 MiB and more as files of their own, an index from extent ID to location, and cleaning of mostly-dead segments. Milestone 3 uploads the same layout to object storage, so the layout should be the one doc-2 gives for it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 A local extent store keeps extents in segment files and files of their own, with an index, and implements the extent-store interface including the root record
- [x] #2 After a crash at any point, it recovers to a prefix of its barrier epochs, shown by a test that kills writes at random points
- [x] #3 Cleaning rewrites segments that are mostly freed extents, and space is reclaimed, shown by a test
- [x] #4 fxdax's Storage over image paths uses it, and fxdax's tests, including cloning and remounting, pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
On branch object-store, after TASK-26.3 (merged to main together).
1. New crate extent_store: a local extent store in a directory, generic over the host filesystem it uses (trait Dir: HostDir on std::fs, MemDir in memory with crash simulation: writes since a file's last fsync, and directory entries since the directory's, survive at random).
   Layout (doc-2's object-storage layout, locally): store (description: format, block size, capacity, branch); segments/<n>: sealed small extents' data packed in commit order, about 32 MiB each; extents/<sequence>.<branch>: extents of 4 MiB and more, a file each; index/<n>: a checkpoint of the index (extent -> pieces in segments or own file, next sequence, root), then a delta per commit (pieces written, extents freed and moved, root), each length-prefixed and hashed.
   Commit = durable barrier: fsync the data files written, append the delta, fsync it; then delete what it made unreferenced (freed large extents, cleaned segments). Recovery replays the newest valid checkpoint and its valid deltas, a prefix of the epochs, and deletes unreferenced files. Write-once: commits seal what they include.
   Cleaning: segments mostly freed have their live extents copied to the open segment, and are deleted after the commit recording the moves.
2. Tests: unit tests of the store; a crash test that kills the store's file system at random points of a random workload and checks that recovery holds exactly the extents and root of some epoch between the last completed commit and the last started one; a cleaning test (space reclaimed, data intact).
3. fxdax: ImageDevice on extent_store's Store<HostDir> (data I/O still through fxdax's Io, into the kernel's transfer buffers); clones copy/share every file of the store; image format 2, no conversion. fxdax's tests, cloning and remounting included, pass. Merge object-store to main once both suites pass on macOS and Linux.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Landed on branch object-store (local, with TASK-26.3; not on main until test_encrypted_mutations_mixed_versions, obsolete with format 60, is deleted with the user's OK): 04d267e crates/extent_store (segments in commit order, large extents' own files, index of checkpoints + hashed per-commit deltas, durable commits, recovery to a prefix of commits, cleaning; HostDir and MemDir with crash simulation); f90b791 copy_store for forks; 4e9c364 fxdax's ImageDevice on it, clones via copy_store, and Journal::renew_tail renews a fork's inherited journal tail; 0439f59 OtherBranch errors, FakeDevice refuses other branches' writes, Device::fork and Fxfs's test_fork. Evidence: AC1 extent_store tests (reads, pieces, large extents, roots, write-once, capacity, frees, checkpoints, copies); AC2 recovers_a_prefix_of_its_commits (300 random crash points, exact epoch match; skipping data or directory syncs makes it fail); AC3 cleaning_in_memory/cleaning_on_the_host; AC4 fxdax's tests (clones of closed and mounted images, remounts) and the libkrun guest e2e tests (4/4 on HVF). Suites: macOS 1859 run 1858 passed, Linux 1863 run 1862 passed; the 1 is the obsolete test, so DoD #1 waits. upstream-changes.py --check passes.

Definition of Done #1 met on 2026-10-23 at 1586daee+: workspace nextest via scripts/test.sh on macOS (2466 passed) and Linux on GCE r63 (2544 passed); Linux tests run on GCE, not Lima, now. The only failure that held this, test_encrypted_mutations_mixed_versions, was removed in 7936796c.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
The local extent store keeps its data in segments on disk; crash recovery to a prefix (crates/extent_store/tests/crash recovers_a_prefix_of_its_commits) and cleaning (tests/store.rs) are tested. Closed in the 2026-10-23 audit of stale tasks once the workspace suite passed on macOS and Linux.
<!-- SECTION:FINAL_SUMMARY:END -->
