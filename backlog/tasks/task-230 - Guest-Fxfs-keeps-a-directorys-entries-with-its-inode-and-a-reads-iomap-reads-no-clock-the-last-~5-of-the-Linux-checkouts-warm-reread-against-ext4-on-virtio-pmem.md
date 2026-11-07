---
id: TASK-230
title: >-
  Guest Fxfs keeps a directory's entries with its inode, and a read's iomap
  reads no clock: the last ~5% of the Linux checkout's warm reread against ext4
  on virtio-pmem
status: To Do
assignee: []
created_date: '2026-11-07 11:30'
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
- [ ] #1 fxfs_dax_iomap_begin and fxfs_dax_iomap_end read the clock only for a write(2)'s iomap, which dax_write_begin_ns and dax_write_end_ns time
- [ ] #2 A directory read from the start serves its kept listing; every change of its names drops it; a follower keeps none; fxfs-guest-checks' listings_follow_changes passes, in both suites, and fails with any one drop removed
- [ ] #3 Measured in one run on GCE (linux-checkout-ext4-pmem and linux-checkout-fxfs-dax): Fxfs DAX's warm reread against pmem's and against r901's 568 ms, and the directory walk's time on each
- [ ] #4 fxfs-guest-checks, pjdfstest and xfstests' directory cases pass on GCE
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 cargo nextest run --workspace passes on macOS and Linux (GCE, scripts/test-gce.sh)
- [ ] #2 scripts/upstream-changes.py --check passes
- [ ] #3 Deviations from upstream documented where they occur
<!-- DOD:END -->
