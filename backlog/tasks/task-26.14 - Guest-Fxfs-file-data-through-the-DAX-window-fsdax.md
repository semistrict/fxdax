---
id: TASK-26.14
title: 'Guest Fxfs: file data through the DAX window (fsdax)'
status: Done
assignee:
  - '@claude'
created_date: '2026-10-14 14:46'
updated_date: '2026-10-14 15:58'
labels: []
milestone: m-3
dependencies:
  - TASK-26.12
  - TASK-26.13
parent_task_id: TASK-26
type: feature
ordinal: 50000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
doc-2's data path, with the user's choice of full DAX (2026-10-14): regular files are DAX inodes when the extent device has a DAX window with DAX_WRITE, as virtio-fs's DAX does. Reads and mmap map extents' ranges from the window (iomap over MAP), so file pages are not in the guest's page cache; writes allocate extents through Fxfs (map-only writes) and store through writable mappings. The driver provides a dax_device over the window. Without a window, files stay in the page cache.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 With a DAX window, files report STATX_ATTR_DAX, and reads, writes, mmap (shared and writable) and truncation pass fxfs-guest's and virtiofs-fxdax's checks
- [x] #2 Reading a file maps it instead of issuing READ requests (the device's request counts)
- [x] #3 pjdfstest and the crash test pass with DAX
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (Lima)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
Design (2026-10-14):
- DAX needs the window's mappings at Fxfs's block size: the device's map_alignment must be at most PAGE_SIZE = Fxfs's 4 KiB block. That holds on Linux hosts with 4 KiB pages (doc-2's target, and Lima's KVM, where the e2e tests run); on 16 KiB hosts (macOS) files stay in the page cache. Fxfs with 16 KiB blocks would need the journal's one-block extents reworked: not in scope.
1. Driver: with DAX, the window is MEMORY_DEVICE_FS_DAX pages (devm_memremap_pages) and a dax_device (as virtio_fs_setup_dax); DAX_WRITE negotiated; exported to fs/fxfs.
2. fs/fxfs/device.c: a seal gate per device: stores through writable mappings (kernel copies, and faults mapping writably) run inside it, and requests that seal (COMMIT, WRITE with a barrier) wait for them, after the filesystem write-protects user mappings and forgets writable window pages (its sealer callback). Synchronous MAP/UNMAP.
3. Rust: ExtentDevice tracks open extents (WRITE, and those Fxfs maps for writes) until the next seal, as Device::is_open, and enters the gate around sealing requests. fxfs_dax_extents (a file range's extents) and fxfs_dax_map_write (txn_map_write and the size, in one transaction).
4. fs/fxfs/dax.c: the window in 2 MiB ranges, each a chunk of one inode's file, with which pages are mapped and which writably; reclaim of unused ranges; iomap ops (holes, mapped, writes moving blocks to new extents and mapping them writably); read_iter/write_iter (copies with page faults disabled inside the gate, faulting the user's pages in first); mmap faults; writepages; truncation (zeroing the tail page, unmapping what is past the end); eviction.
5. super.c: regular files are DAX inodes (S_DAX) when the device has a window with DAX_WRITE and map_alignment <= PAGE_SIZE.
6. Tests: fxfs-guest-dax (libkrun): fxfs-guest's checks with a DAX window, STATX_ATTR_DAX, reading a file adds no READ requests; pjdfstest and the crash test with DAX. Run on Linux (Lima KVM).
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Validation (Linux, Lima KVM, 4 KiB pages): fxfs-guest-dax (STATX_ATTR_DAX; reading a 3 MiB file after eviction: 3 MAPs, one per extent, no READ), fxfs-guest-checks-dax (virtiofs-fxdax's checks: shared writable mmap, writes visible through mappings, truncation zeroing a mapped tail), fxfs-guest-crash-dax, pjdfstest-fxfs-guest-dax (238 files, 8798 tests, PASS); non-DAX fxfs-guest, -format, -crash, -checks, extent-guest, pjdfstest-fxfs-guest pass on macOS and Linux. scripts/test.sh 1865/1865, scripts/test-linux.sh 1869/1869, upstream-changes.py --check passes. Commits 16735de, c552aee; libkrun ec3e8ea. On 16 KiB hosts (macOS) files stay in the page cache: DAX there needs Fxfs blocks of 16 KiB, which needs the journal's one-block extents reworked.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Regular files are DAX inodes when the extent device's window maps open extents writably at the page size: reads map extents from the window, writes make room in open extents and store through writable mappings, with a device gate that keeps sealing requests out of in-progress stores. Verified by fxfs-guest-dax, fxfs-guest-checks-dax, fxfs-guest-crash-dax and pjdfstest with DAX on Linux KVM, plus the host suites.
<!-- SECTION:FINAL_SUMMARY:END -->
