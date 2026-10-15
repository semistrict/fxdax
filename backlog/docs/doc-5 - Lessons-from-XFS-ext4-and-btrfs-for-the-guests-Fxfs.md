---
id: doc-5
title: 'Lessons from XFS, ext4 and btrfs for the guest''s Fxfs'
type: other
created_date: '2026-10-15 08:29'
updated_date: '2026-10-15 08:29'
---
# Lessons from XFS, ext4 and btrfs for the guest's Fxfs

Read from Linux 7.3-rc3 (~/src/linux, fs/xfs, fs/ext4, fs/btrfs) on 2026-10-15, for TASK-26.18 (performance) and TASK-26.19 (features). File:line references are to that tree.

## Metadata: keep the directory-lock critical section short

XFS does nothing special about the directory's i_rwsem: `xfs_vn_unlink` (xfs_iops.c:403) runs `xfs_remove` (xfs_inode.c:1912) under it. What runs there is short and never sleeps: lockless log reservation, ILOCKs, cached-buffer edits, and a commit into the CIL under a shared lock. In a 2-vCPU guest, a spinning i_rwsem waiter burns half the machine, so hold time is the lever. Fxfs's unlink holds the lock across transaction building, the commit mutex, journal serialization and skip-list apply.

1. **Cheap, concurrent commits (CIL, xfs_log_cil.c).**
   - Shadow buffers are allocated and sized before any lock (:268, :1830-1835).
   - Committers take `xc_ctx_lock` shared (:1838) and insert into per-CPU lists (:633-748). Lists are merged and sorted only at push time (:119, :1501).
   - The push runs asynchronously on a workqueue; committers block only past 2x the soft limit (:1629-1689).
   - For Fxfs: serialize a transaction's mutations before the commit mutex, and under it only reserve a journal position and copy. Apply to the skip list outside the global mutex, ordered by sequence.
2. **Log space ahead of need.**
   - Grant heads are lockless (xfs_log.c:250-280, :92-96).
   - The AIL is pushed to keep 25% of the log free (xfs_trans_ail.c:398-466).
   - For Fxfs: start journal flush and compaction early, and make the journal-space check an atomic fast path.
3. **Background inactivation.**
   - Unlink only removes the name, drops nlink, and links the inode into one of 64 unlinked buckets per AG (libxfs/xfs_inode_util.c:461-541).
   - Inactivation runs from per-CPU queues, batched (xfs_icache.c:2174-2218, :1952); `evict_inode` is trivial.
   - For Fxfs, the graveyard is this design. Shard its key, batch the reaper per CPU, and keep evict free of transactions.
4. **Same-object aggregation.** An object changed again while in the CIL overwrites its logged copy (xfs_log_cil.c:578-597); it is logged once per checkpoint. For Fxfs: coalesce repeated same-key records (a shared directory's attributes) within a flush window.
5. **Per-CPU counters and per-AG partitioning.** percpu_counter superblock counters (xfs_trans.c:588-637), an RCU radix-tree inode cache per AG, directories rotated across AGs. For Fxfs: per-CPU object-id ranges and counters.
6. **fsync by sequence.** fsync forces the log only up to the inode's last commit (xfs_file.c:102-121). Concurrent fsyncs share one force.
7. **btrfs delayed items** (fs/btrfs/delayed-inode.c): directory-index and inode-item changes are queued in memory and applied to the B-tree in batches by workers. This is the copy-on-write tree's answer to per-op tree cost.
8. **SB_NOSEC:** given by `get_tree_bdev`, and set explicitly by btrfs (super.c:1896). Without it every write looks up security xattrs.

## DAX (XFS)

- **One fault handler for PTE and PMD:** `.huge_fault` and `.fault` both go to `dax_iomap_fault` (xfs_file.c:2077-2105), with `thp_get_unmapped_area` (:2172).
  - PMD falls back to PTE when the colour doesn't match, on a private write, when it would cross the VMA or EOF, or when iomap_begin returns less than 2 MiB (dax.c:1979-2010, :2071).
  - For Fxfs, iomap_begin must return 2 MiB of aligned window, so large extents need a 512-page MAP.
- **MAP_SYNC:** `FOP_MMAP_SYNC` (xfs_file.c:2176). iomap_begin sets `IOMAP_F_DIRTY` only when the inode has uncommitted metadata (xfs_iomap.c:159-166, :879-880), which leads to `dax_finish_sync_fault`. Overwrites need no transaction.
- **No delalloc or unwritten extents on DAX.** Allocation zeroes before commit (xfs_iomap.c:302-322), and is amortized up to 1024 pages and the extsize hint (:1003-1007, :232-289).
- **Layout breaking:** MMAPLOCK is the invalidate_lock. `xfs_break_dax_layouts` waits out pinned pages (xfs_inode.c:3006-3024) before truncate, fallocate, remap and evict.
- **Copy-on-write on DAX:**
  - iomap is the new blocks with `IOMAP_F_SHARED`, and `srcmap` is the old (xfs_iomap.c:1021-1032).
  - `dax_iomap_copy_around` copies before inserting the writable PFN (dax.c:1882-1885).
  - The extent swap happens in iomap_end (xfs_iomap.c:1296-1302).
  - This is Fxfs's write-to-sealed-extent path.
- **Shared pages:** read iomaps set `IOMAP_F_SHARED` (xfs_iomap.c:2213-2222), so DAX tracks folios mapped by several files (dax.c:345-378, 478-504).

## Reflink (XFS) and a design for Fxfs (TASK-26.19.6)

- **Entry and locking:**
  - `xfs_file_remap_range` (xfs_file.c:1697-1769) accepts only DEDUP|CAN_SHORTEN.
  - It locks both inodes in pointer order with break_layout retries, then MMAPLOCK or `filemap_invalidate_lock_two` plus a DAX layout break on both (xfs_inode.c:2700-2815).
  - It calls `dax_remap_file_range_prep` / `generic_remap_file_range_prep` (remap_range.c:276-365).
- **Remap loop:**
  - The source is flagged `XFS_IREMAPPING` so its writers wait (xfs_file.c:202-210).
  - The remap runs one transaction per extent (xfs_reflink.c:1348-1637) through intents, so a crash leaves a prefix.
  - copy_file_range calls remap_file_range with CAN_SHORTEN (read_write.c:1598-1600). Dedupe compares under the locks (dax.c:2196-2271).
- **For Fxfs:**
  1. Lock as XFS does, and break DAX layouts on both inodes.
  2. Call `dax_remap_file_range_prep` with Fxfs's read iomap ops.
  3. Seal any open staging extent in the source range.
  4. Zero past EOF and truncate the destination's page cache over the range.
  5. Per run of source extents, one transaction: drop the destination's extents, add references to the source's, update the size.
  6. Flag the source as remapping while this runs.
  7. Set `IOMAP_F_SHARED` on iomaps of shared extents.

## VFS features missing in Fxfs (TASK-26.19)

O_TMPFILE, fallocate, fiemap, fileattr, POSIX ACLs, remap_file_range, freeze/unfreeze, show_options, shutdown, `fs_flags` (FS_MGTIME, FS_ALLOW_IDMAP) and SB_I_VERSION, export_operations, shrinker hooks for cached objects, large folios, O_DIRECT, and MAP_SYNC.
