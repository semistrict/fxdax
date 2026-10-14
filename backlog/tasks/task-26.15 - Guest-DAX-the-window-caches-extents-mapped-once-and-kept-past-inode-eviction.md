---
id: TASK-26.15
title: 'Guest DAX: the window caches extents, mapped once and kept past inode eviction'
status: In Progress
assignee:
  - '@claude'
created_date: '2026-10-14 23:10'
updated_date: '2026-10-14 23:11'
labels: []
dependencies: []
parent_task_id: TASK-26
ordinal: 51000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The guest's DAX window is split into 2 MiB ranges, each mapping a chunk of one file, unmapped when its inode is evicted: a file read again after its inode is evicted maps its extents again (MAP requests), and a small file would take a whole range. ext4 on writable pmem reads storage as memory, pages staying mapped across the guest's cache drops. The user requires one copy of data in memory (no guest page cache copy beside the host's), and that every VM's mappings share the host's pages (doc-2): the host side, pager-backed VMOs per sealed extent copied from Fuchsia's FxBlob, is done (4738f2b).
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Each sealed extent's pages are one copy in memory, the host's, whichever files and windows map it (a window may map an extent at more than one address)
- [ ] #2 A file's window mappings outlive its inode: dropped when its data is freed or moved, or least recently used when the window needs room
- [ ] #3 Reading a DAX file whose inode was evicted, while its ranges are still mapped, takes no device request (a test counts the device's MAP and READ requests)
- [ ] #4 A small file's range takes the window space its size needs, not 2 MiB
- [ ] #5 pjdfstest and the fxfs-guest DAX tests pass
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Research: fsdax shares a page between files only reflink-style (IOMAP_F_SHARED, copy-on-write), changing a file page's address as its block moves; dax.c's per-file ranges keep each file page at one window address, remapping moved blocks there, invisibly to fsdax. Keep that, and:
1. Ranges belong to a file (inode number) rather than an inode: a global table (inode number -> the file's ranges, by chunk, and its DAX semaphore) that an inode attaches to as it is read in, and that outlives it.
2. Evicting an inode detaches it; its ranges stay mapped. Evicting an unlinked one, or truncation, unmaps them.
3. Window space from a page-granular allocator (genalloc): a range is as large as its chunk of the file, rounded to pages, up to 2 MiB; bitmaps sized to it. A range that a write or truncation would grow past its size is dropped (released and unmapped) under the inode's locks, and mapped again sized anew.
4. Reclaim takes the least recently used unused range: of a file with no inode, unmapped at once; else as now (release the inode's DAX entries, then unmap).
5. Test: a DAX file evicted and read again takes no MAP or READ request; and a small DAX file's range is one page.
<!-- SECTION:PLAN:END -->
