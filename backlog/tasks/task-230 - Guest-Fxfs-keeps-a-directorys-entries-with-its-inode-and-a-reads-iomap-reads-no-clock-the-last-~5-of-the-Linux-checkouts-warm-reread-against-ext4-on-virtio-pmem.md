---
id: TASK-230
title: >-
  Guest Fxfs keeps a directory's entries with its inode, and a read's iomap
  reads no clock: the last ~5% of the Linux checkout's warm reread against ext4
  on virtio-pmem
status: Done
assignee:
  - '@claude'
created_date: '2026-11-07 11:30'
updated_date: '2026-11-07 14:39'
labels: []
dependencies: []
references:
  - TASK-227
  - TASK-136.1
  - guest/linux/fs/fxfs/super.c
  - guest/linux/fs/fxfs/dax.c
priority: medium
type: enhancement
ordinal: 665000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Split off TASK-227 (the user, 2026-11-07): TASK-227's target is met at 1.05-1.06x of ext4 on virtio-pmem (GCE r901, r902), and this closes the rest without holding it up.

Where the gap is (r901, the warm reread of the 1.54 GB Linux tree, 91k files and 6,059 directories, profiled in the reader alone at 10 kHz): the copy costs the same on both filesystems (pmem's copy_mc_enhanced_fast_string 2,285 samples, Fxfs DAX's rep_movs_alternative 2,339). Fxfs DAX takes 641 samples more (7.6%):
- About 520-610 in its directory reads. Each readdir builds a merger over every layer of the LSM tree, seeks each, and decodes records with bincode (persistent_layer, MergerIterator, bincode, __kvmalloc_node). Each directory open also allocates a 32 KiB buffer (fxfs_vfs_iterate's cursor). ext4's htree readdir costs about 155.
- About 190-210 in down_read.
- About 160 in clock reads (read_tsc, ktime_get): fxfs_dax_iomap_begin and fxfs_dax_iomap_end each read the clock on every iomap, though only a write(2)'s uses it.

The caller attribution is inferred from the symbols; r902's call-chain reports failed on perf's addr2line. r904 (FXDAX_LINUX_CHECKOUT_PROFILE, walk-warm and open-warm) times the directory walk and the opens apart.

Work in progress on branch t227-prof:
- 23b22c735: a read's iomap reads no clock. r902 measured it: Fxfs DAX's fresh-VM warm reread 564 ms against 568-578 ms without, pmem's 536 ms.
- fe18c78f6: a directory's whole listing, when it fits one 32 KiB read, kept with its inode under i_lock (fxfs_vfs_dir_listing). It is taken by a read from the start, kept by a read that read all of it under the directory's shared lock, and dropped as any change begins (create, mkdir, mknod, symlink, link, unlink, rmdir, rename, exchange), all of which hold the lock exclusively, and at eviction. Followers keep none. Its guest check is listings_follow_changes (fxfs-guest-checks). Not yet built or run.
- The measurement commits there (FXDAX_LINUX_CHECKOUT_PROFILE in test_linux_checkout.rs) are for measuring only.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 fxfs_dax_iomap_begin and fxfs_dax_iomap_end read the clock only for a write(2)'s iomap, which dax_write_begin_ns and dax_write_end_ns time
- [x] #2 A directory read from the start serves its kept listing; every change of its names drops it; a follower keeps none; fxfs-guest-checks' listings_follow_changes passes, in both suites
- [x] #3 Representative invalidations mutated (create, unlink, rename, exchange), each killed
- [x] #4 Measured in one run on GCE (linux-checkout-ext4-pmem and linux-checkout-fxfs-dax): Fxfs DAX's warm reread against pmem's and against r901's 568 ms, and the directory walk's time on each
- [x] #5 fxfs-guest-checks, pjdfstest and xfstests' directory cases pass on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [x] #2 scripts/upstream-changes.py --check passes
- [x] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. A read's iomap reads the clock only for a write(2) (dax.c).
2. A directory's whole listing kept with its inode under i_lock, served to reads from the start, kept by a whole read under the shared lock, dropped as each change begins and at eviction; none for followers (super.c, vfs.h). Check listings_follow_changes.
3. Guest mutants of the four kinds of drop (create, unlink, rename, exchange), killed by the check on one release host (scripts/guest-mutants-gce.py).
4. Measure against ext4 on pmem (linux-checkout pair) and run the checks suites, pjdfstest and xfstests on GCE.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
r904 (GCE, with the clock fix, before the listing cache): the warm reread's parts, timed apart, fresh VM, ext4 on pmem / Fxfs DAX: directory walk alone (walk-warm) 83 / 108 ms; walk and each file opened and closed (open-warm) 295 / 295 ms, so opens alone 213 / 188; whole reread 527 / 555 ms, so reads alone 232 / 260. The gap (+28 ms) is the walk's +25 ms, which the listing cache addresses, and the reads' +28 ms (per read(2): iomap, down_read of the file's semaphore), less the opens' -25 ms. Sandbox-1: walk 111, open-warm 304, reread 565 ms.

r905 (GCE, t227-prof fe18c78f6: the listing cache and the clock fix): fresh VM, ext4 on pmem / Fxfs DAX: walk-warm 84 / 62.5 ms, open-warm 279 / 249 ms, warm reread 530 / 495 ms (0.93x of pmem; r904 without the cache 555 ms, 1.05x); sandbox-1 503 ms, walk 65 ms. listings_follow_changes passed in fxfs-guest-checks and -dax. namespace failed in both only because FXDAX_CHECKS left out regular_write, whose file 'written' its symlink names (namespace.rs:18), not the cache.

r920 (GCE fxdax-test-r881-b, one release kernel, fs/fxfs rebuilt per mutant; scripts/guest-mutants-gce.py), each mutant killed by fxfs-guest-checks listings_follow_changes at the assertion its change breaks (namespace.rs):
- task-230-create-keeps-listing (no drop in fxfs_vfs_mknod: create, mkdir, mknod): line 63, "listed" read after a and b were made.
- task-230-unlink-keeps-listing (no drop in fxfs_vfs_unlink: files and rmdir): line 69, after a's unlink.
- task-230-rename-keeps-listing (fxfs_vfs_rename drops for an exchange alone): line 75, after b's rename to b2.
- task-230-exchange-keeps-listing (fxfs_vfs_rename drops for a rename alone): line 103, listed-too's c linking e's inode after RENAME_EXCHANGE.
fxfs_vfs_mknod's drop moved to its top before r920, where it doesn't break task-79.29's mutant's context; scripts/guest-mutants-gce.py --check: 51 checked, 0 broken, on main 7d332b5f2 too. r905 and r906 ran with it after fxfs_vfs_new_gid, the same drop before any change.

Validation: CI 42379606164 green on land-w2 97e094823 (workspace, guest, sim, shuttle, firecracker, go, clippy scripts with guest-mutants-gce.py --check, guest-sim); upstream-changes.py --check passes on main 97e094823. TASK-230 changes no crate of the workspace (guest/linux/fs/fxfs, guest/tests, guest/mutants): the macOS workspace isn't rerun; it passed 2088/2088 on land-w1's code. Its changes are fxdax's own glue (fs/fxfs), no upstream Fuchsia source.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Two costs off the guest's Fxfs DAX reads, the last of TASK-227's gap to ext4 on virtio-pmem.

- A read's iomap no longer reads the clock: only a write(2)'s, which dax_write_begin_ns and dax_write_end_ns time, does. That was 1-2% of the Linux tree's warm reread (r901, r902).
- A directory's whole listing is kept with its inode while the kernel caches it (fxfs_vfs_dir_listing), as ext4's directory blocks are in the page cache. A read from the start takes it instead of a merger over the LSM tree's layers, record decoding and a 32 KiB cursor buffer. It is kept by a read of all of it under the directory's shared lock, dropped as any change of its names begins (each holding the lock exclusively) and at eviction; a follower keeps none.

Measured on GCE: the tree's 6,059 directories walk warm in 62 ms, from 108 (pmem 83), and the warm reread takes 495 ms, from 555, against pmem's 530 (0.93x, r904, r905).

Verified: the check listings_follow_changes; mutants of the create, unlink, rename and exchange drops, each killed by it (r920); fxfs-guest-checks (and -dax, -dax-small), pjdfstest-fxfs-guest (and -dax), xfstests-fxfs-guest-dax (r906); CI 42379606164.
<!-- SECTION:FINAL_SUMMARY:END -->
