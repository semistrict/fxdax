---
id: TASK-26.10
title: 'Local extent store: segments on disk; fxdax''s images become extent stores'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-13 22:51'
updated_date: '2026-10-14 02:52'
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
- [ ] #1 A local extent store keeps extents in segment files and files of their own, with an index, and implements the extent-store interface including the root record
- [ ] #2 After a crash at any point, it recovers to a prefix of its barrier epochs, shown by a test that kills writes at random points
- [ ] #3 Cleaning rewrites segments that are mostly freed extents, and space is reclaimed, shown by a test
- [ ] #4 fxdax's Storage over image paths uses it, and fxdax's tests, including cloning and remounting, pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
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
