// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Files' data mapped from the extent-store device's DAX window (backlog doc-2), as virtio-fs's DAX
 * maps a host's files (fs/fuse/dax.c), instead of the page cache: reads, writes and mappings of
 * files reach the extents the host shares, through the window.
 *
 * The window is split into ranges, each mapping a chunk of FXFS_DAX_SIZE bytes of one file: the
 * extents holding the chunk's data, page by page, read-only, or writably for open extents (doc-4),
 * and holes left out. A range is as large as its chunk of the file, in pages: a small file's takes
 * what it needs of the window, rounded up to a power of two, so that a file growing a page at a
 * time maps its data again only as often as it doubles. A write first makes room for its data in
 * open extents (fxfs_dax_map_write), which the range then maps writably, and stores through the
 * mapping; mappings of files map the window's pages. A file's page stays at one window address,
 * whatever extent holds it: a block that moves is mapped again there.
 *
 * A chunk whose data is one piece of an extent of at most a chunk's size, as small files' are,
 * sharing extents (the allocator's staging extents), is a slice of that extent's region instead:
 * the window's mapping of the whole extent, from its start, which all the files it holds share. So
 * a file written, or read, takes no request of its own, once its extent is mapped: a region of an
 * open extent is mapped writably, further than its blocks are handed out yet, doubling. A region
 * goes with its last slice. A write a slice can't take where it is, as its blocks move, drops the
 * slice (-EXDEV), and the chunk is mapped again.
 *
 * So too a range's pages whose blocks are in such extents, as blocks a write moves to the staging
 * extent are: the range maps them where their extent's region does (fxfs_dax_move), rather than
 * in its own space, so that a block moved takes no MAP of its own, and the window maps each extent
 * once. A page that moves so is at another address: the iomaps of it after it moved say it is
 * shared (IOMAP_F_SHARED), so that the kernel replaces its mappings of the page.
 *
 * Ranges belong to a file (struct fxfs_dax_file, by inode number), not to an inode: they outlive
 * the inodes read in for the file, so that a file read again maps from the window what it mapped
 * before, with no request, as pmem stays mapped across the guest's dropped caches. A file's ranges
 * go as its data does (unlinked, truncated); others are reclaimed, least recently used, when the
 * window runs out.
 *
 * Requests that seal open extents make their writable mappings read-only. Stores are made in the
 * device's gate (device.h), which such requests wait for; before them, the sealer makes user
 * mappings of writable pages read-only, so that a store faults, and forgets which pages are
 * writable, so that a write makes room again. Kernel copies through the window are made with page
 * faults disabled, as one waiting for the gate to open may hold what a seal needs.
 *
 * Locks are taken in this order: an inode's lock (read and write) or its invalidate_lock (faults,
 * truncation and reclaim), then its file's DAX semaphore, then the gate. Another inode's are only
 * tried, when reclaiming its ranges. Rust is called holding no gate.
 */

#include <linux/dax.h>
#include <linux/genalloc.h>
#include <linux/hash.h>
#include <linux/hashtable.h>
#include <linux/iomap.h>
#include <linux/log2.h>
#include <linux/mm.h>
#include <linux/pagemap.h>
#include <linux/pfn_t.h>
#include <linux/slab.h>
#include <linux/uio.h>
#include <linux/wait.h>
#include <linux/workqueue.h>
#include <linux/writeback.h>

#include "device.h"
#include "vfs.h"

/* A range's size: a power of two, the window's and chunks' alignment. */
#define FXFS_DAX_SHIFT 21
#define FXFS_DAX_SIZE (1ULL << FXFS_DAX_SHIFT)
#define FXFS_DAX_PAGES (FXFS_DAX_SIZE >> PAGE_SHIFT)

/*
 * The window's space is handed out from a pool, page by page, at this offset: the pool takes no
 * address 0.
 */
#define FXFS_DAX_POOL_BASE (1UL << 48)

/* The most pages a writable region's mapping grows by at once. */
#define FXFS_DAX_REGION_STEP 64UL
/*
 * The pages a writable region's first MAP maps: most of a millisecond each, which a small file's
 * write waits for, so more than a page, though they make the extent at least that long.
 */
#define FXFS_DAX_REGION_FIRST 16UL

/* The window's mapping of a whole extent, from its start, which slices of files share. */
struct fxfs_dax_region {
	/* In the filesystem's regions, by extent. */
	struct hlist_node node;
	u64 branch;
	u64 sequence;
	u64 window_offset;
	/* Its space in the window, in pages: as far as the extent reaches, or may yet. */
	unsigned long pages;
	/*
	 * Its slices, the ranges whose pages it maps (fxfs_dax_move), and its growth. Guarded by the
	 * regions lock.
	 */
	unsigned int refs;
	/* Guards `mapped`, and the region's MAPs. */
	struct mutex lock;
	/* How many of its pages are mapped, from its start. */
	unsigned long mapped;
	/*
	 * Whether it maps its extent writably, while it is open: with the gate closed, a seal makes it
	 * read-only. Set in the gate, under the regions lock.
	 */
	bool writable;
	/* On the filesystem's writable regions, while it is. Guarded by the regions lock. */
	struct list_head writable_list;
	/*
	 * Maps a writable region further ahead of the writes that will use it, so that they don't
	 * wait for the MAP: while `growing`, it holds a reference to the region.
	 */
	struct fxfs_dax *dax;
	struct work_struct grow;
	atomic_t growing;
	/* The furthest page writes reached, which the region grows ahead of. */
	atomic_long_t reached;
};

/* A file's ranges of the window, which outlive its inodes. */
struct fxfs_dax_file {
	u64 ino;
	/* The filesystem's (`files`), and others' while they use it. */
	refcount_t refs;
	/* The inode read in for the file, if there is one. Guarded by the filesystem's files lock. */
	struct inode *inode;
	/* Guards `ranges`, and which pages of them are mapped. */
	struct rw_semaphore sem;
	/* The window's ranges mapping chunks of the file, by chunk. */
	struct xarray ranges;
	/* On the filesystem's list of files with writable window pages, if it has some. */
	struct list_head writable;
};

/* A range of the window. */
struct fxfs_dax_range {
	/* On the filesystem's used list. */
	struct list_head list;
	/* The file whose chunk it maps, and which. */
	struct fxfs_dax_file *file;
	pgoff_t chunk;
	u64 window_offset;
	/* The region it is a slice of, whose pages it maps, if it is one. */
	struct fxfs_dax_region *region;
	/* Its size, in pages: its chunk's, up to the file's end when it was mapped. */
	unsigned long pages;
	/* The iomaps using it, which keep it. */
	atomic_t users;
	/* Its pages that map an extent, and those that map one writably. */
	unsigned long *mapped;
	unsigned long *writable;
	/*
	 * A range of its own's pages mapped where their extents' regions map them (fxfs_dax_move):
	 * for each page, 0 if it is in the range's own space, or else the index of its region in
	 * `regions` plus one, shifted 32 bits, and its page in the region. Made as a page first moves.
	 */
	u64 *moved;
	/*
	 * The regions its pages moved to, each of which it holds a reference to while it is: iomaps
	 * may use a page where it was until they end, and the kernel's mappings of it until the
	 * kernel replaces them.
	 */
	struct fxfs_dax_region **regions;
	unsigned int nr_regions;
	/*
	 * Its pages at another address than the iomaps of them said before, which the next iomaps
	 * say are shared (fxfs_dax_cow).
	 */
	unsigned long *relocated;
	/*
	 * Freed after a grace period: the sealer walks files' ranges without their semaphores,
	 * which writers hold waiting for the gate the sealer keeps closed.
	 */
	struct rcu_head rcu;
};

struct fxfs_dax {
	struct super_block *sb;
	struct fxfs *fs;
	struct fxfs_device *dev;
	struct dax_device *dax_dev;
	/* The window's free space. */
	struct gen_pool *pool;
	/* The files with ranges, or inodes using the window, by inode number. */
	struct xarray files;
	/* Guards `files`, and files' inodes. */
	struct mutex files_lock;
	/* Guards the used list. */
	spinlock_t lock;
	/* The ranges mapping chunks, least recently used first. */
	struct list_head used;
	/* Woken as ranges are freed, or their last users done, which counts in `released`. */
	wait_queue_head_t wait;
	atomic_long_t released;
	/* Guards `writable`: the files with writable pages in the window. */
	struct mutex writable_lock;
	struct list_head writable;
	/* The regions mapping extents, by extent. */
	DECLARE_HASHTABLE(regions, 12);
	/* Guards `regions`, regions' references, and the writable regions. */
	spinlock_t regions_lock;
	struct list_head writable_regions;
	/* Where writable regions grow ahead of their writes (fxfs_dax_grow), and how many do. */
	struct workqueue_struct *grow_wq;
	atomic_t growing;
};

static const struct iomap_ops fxfs_dax_iomap_ops;
static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start);
static int fxfs_dax_reach(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end);
static void fxfs_dax_grow(struct work_struct *work);
static int fxfs_dax_move(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			 const struct fxfs_dax_extent *e, unsigned long page, unsigned long pages);

static struct fxfs_dax *fxfs_dax_of(struct inode *inode)
{
	return fxfs_vfs_info(inode->i_sb)->dax;
}

/* The file `inode` is read in for, whose ranges it uses. */
static struct fxfs_dax_file *fxfs_dax_file(struct inode *inode)
{
	return fxfs_vfs_inode(inode)->dax.file;
}

static loff_t fxfs_dax_chunk_start(struct fxfs_dax_range *range)
{
	return (loff_t)range->chunk << FXFS_DAX_SHIFT;
}

/* Room for the pieces of a chunk's data, at most one per page, from Rust. */
static struct fxfs_dax_extent *fxfs_dax_alloc_extents(void)
{
	return fxfs_nofs(kvmalloc_array(FXFS_DAX_PAGES, sizeof(struct fxfs_dax_extent),
					GFP_KERNEL));
}

/* Marks `file` as having writable pages in the window, which the sealer forgets. In the gate. */
static void fxfs_dax_add_writable(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	mutex_lock(&dax->writable_lock);
	if (list_empty(&file->writable))
		list_add_tail(&file->writable, &dax->writable);
	mutex_unlock(&dax->writable_lock);
}

/*
 * Takes the pieces `extents` of the chunk the slice `range` maps where it maps them, writably those
 * of its region's extent while it is open, which is mapped as far as they reach; or -EXDEV if they
 * are elsewhere, as moved blocks are, which the slice can't map. In the gate, with the file's DAX
 * semaphore held for writing.
 */
static int fxfs_dax_map_slice(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			      const struct fxfs_dax_extent *extents, ssize_t count)
{
	struct fxfs_dax_region *region = range->region;
	loff_t start = fxfs_dax_chunk_start(range);
	/* Where the slice starts in its extent. */
	u64 base = range->window_offset - region->window_offset;
	bool writable = false;
	ssize_t i;
	int err;

	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];
		unsigned long page = (e->offset - start) >> PAGE_SHIFT;
		unsigned long pages = e->len >> PAGE_SHIFT;
		u64 end = base + (e->offset - start) + e->len;

		if (e->branch != region->branch || e->sequence != region->sequence ||
		    e->extent_offset != base + (e->offset - start) || page + pages > range->pages ||
		    (e->open && !region->writable))
			return -EXDEV;
		if (!e->open)
			continue;
		err = fxfs_dax_reach(dax, region, end >> PAGE_SHIFT);
		if (err)
			return err;
		bitmap_set(range->writable, page, pages);
		writable = true;
	}
	if (writable)
		fxfs_dax_add_writable(dax, range->file);
	return 0;
}

/* Where `range`'s page `page` is in the window: in its own space, or its region's (`moved`). */
static u64 fxfs_dax_page_offset(const struct fxfs_dax_range *range, unsigned long page)
{
	u64 moved = range->moved ? range->moved[page] : 0;

	if (!moved)
		return range->window_offset + ((u64)page << PAGE_SHIFT);
	return range->regions[(moved >> 32) - 1]->window_offset + ((moved & U32_MAX) << PAGE_SHIFT);
}

/* How many of `range`'s pages from `page` on, before `end`, are in a row in the window. */
static unsigned long fxfs_dax_run(const struct fxfs_dax_range *range, unsigned long page,
				  unsigned long end)
{
	u64 at = fxfs_dax_page_offset(range, page);
	unsigned long next;

	if (!range->moved)
		return end - page;
	for (next = page + 1; next < end; next++) {
		if (fxfs_dax_page_offset(range, next) != at + ((u64)(next - page) << PAGE_SHIFT))
			break;
	}
	return next - page;
}

/*
 * Has `range`'s `pages` pages from `page` on be where `region`'s pages from `first` on are, or in
 * the range's own space without `region`: those the window mapped elsewhere before are
 * relocated, so that the kernel's mappings of them go (fxfs_dax_cow). With the file's DAX
 * semaphore held for writing.
 */
static void fxfs_dax_relocate(struct fxfs_dax_range *range, unsigned long page,
			      unsigned long pages, struct fxfs_dax_region *region, unsigned long first)
{
	unsigned int index = 0;
	unsigned long i;
	u64 moved;

	if (!range->moved)
		return;
	if (region) {
		while (range->regions[index] != region)
			index++;
	}
	for (i = 0; i < pages; i++) {
		moved = region ? ((u64)(index + 1) << 32) | (first + i) : 0;
		if (range->moved[page + i] != moved && test_bit(page + i, range->mapped))
			set_bit(page + i, range->relocated);
		range->moved[page + i] = moved;
	}
}

/*
 * Has `iomap` say its pages are shared, if any of `range`'s `pages` pages from `page`, which it
 * maps, was relocated since the iomaps before: so that the kernel replaces its mappings of them
 * (dax_insert_entry), and its DAX entries (dax_iomap_iter, dax_zero_iter), copying nothing:
 * `srcmap`, the data around a write, is where the pages are already. Fills in `srcmap`, so after
 * `iomap` is. (That copy of the pages onto themselves isn't atomic: a store racing it, through
 * another mapping of a page just relocated, or a write(2) racing a fault, may be undone. Only
 * on pages relocated since the iomaps before, and as racing stores to one page are in any case.)
 */
static void fxfs_dax_cow(struct fxfs_dax_range *range, unsigned long page, unsigned long pages,
			 struct iomap *iomap, struct iomap *srcmap)
{
	unsigned long i;
	bool relocated = false;

	if (find_next_bit(range->relocated, page + pages, page) == page + pages)
		return;
	/* Atomically: read iomaps take the file's semaphore for reading. */
	for (i = page; i < page + pages; i++)
		relocated |= test_and_clear_bit(i, range->relocated);
	if (!relocated)
		return;
	iomap->flags |= IOMAP_F_SHARED;
	*srcmap = *iomap;
	srcmap->flags = 0;
	srcmap->private = NULL;
}

/*
 * Maps the pieces `extents` of the chunk `range` maps, at their places in it: open extents
 * writably. A range of its own maps a piece of an extent no larger than a chunk where the
 * extent's region does, if it can (fxfs_dax_move). In the gate if any is open, with the file's DAX
 * semaphore held for writing.
 */
static int fxfs_dax_map_extents(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				const struct fxfs_dax_extent *extents, ssize_t count)
{
	loff_t start = fxfs_dax_chunk_start(range);
	bool writable = false;
	ssize_t i;
	int err;

	if (range->region)
		return fxfs_dax_map_slice(dax, range, extents, count);
	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];
		unsigned long page = (e->offset - start) >> PAGE_SHIFT;
		unsigned long pages = e->len >> PAGE_SHIFT;

		err = -EXDEV;
		if (e->extent_size && e->extent_size <= FXFS_DAX_SIZE &&
		    PAGE_ALIGNED(e->extent_offset))
			err = fxfs_dax_move(dax, range, e, page, pages);
		/*
		 * No room for the region now, nor memory to track the page there, or it can't map the
		 * piece: the range's own space does.
		 */
		if (err == -EXDEV || err == -EAGAIN || err == -ENOSPC || err == -ENOMEM) {
			err = fxfs_device_map(dax->dev, e->branch, e->sequence, e->extent_offset,
					      e->len, range->window_offset + (e->offset - start),
					      e->open);
			if (!err)
				fxfs_dax_relocate(range, page, pages, NULL, 0);
		}
		/*
		 * A sealed extent the device doesn't hold was never written: its blocks read as
		 * zeros, a hole, as Fxfs reads them. (Blocks a write allocated before a seal sealed
		 * their extent unwritten, which the write then moves.)
		 */
		if (err == -ENOENT && !e->open)
			continue;
		if (err) {
			pr_warn("fxfs: mapping %llu bytes at %llu of extent %llx:%llu, %s, at %llu of the DAX window failed: %d\n",
				e->len, e->extent_offset, e->branch, e->sequence,
				e->open ? "writably" : "read-only",
				range->window_offset + (e->offset - start), err);
			return err;
		}
		bitmap_set(range->mapped, page, pages);
		if (e->open) {
			bitmap_set(range->writable, page, pages);
			writable = true;
		} else {
			bitmap_clear(range->writable, page, pages);
		}
	}
	if (writable)
		fxfs_dax_add_writable(dax, range->file);
	return 0;
}

/* Whether any of the pieces `extents` is of an open extent. */
static bool fxfs_dax_any_open(const struct fxfs_dax_extent *extents, ssize_t count)
{
	ssize_t i;

	for (i = 0; i < count; i++) {
		if (extents[i].open)
			return true;
	}
	return false;
}

/*
 * Enters the device's gate (fxfs_device_store_begin), and returns the seal count: or, for a fault,
 * -EBUSY rather than wait for a seal in progress. A fault holds its page's DAX entry, which a
 * write(2) in the gate may wait for (dax_iomap_iter invalidating the page's entries), while the
 * seal waits for that write: the fault lets go of its entry and waits for the seal instead
 * (fxfs_dax_do_fault), then tries again.
 */
static long fxfs_dax_store_begin(struct fxfs_dax *dax, bool fault)
{
	u64 seals;

	if (!fault)
		return fxfs_device_store_begin(dax->dev);
	return fxfs_device_store_try_begin(dax->dev, &seals) ? (long)seals : -EBUSY;
}

/*
 * Maps the extents holding the data of the chunk `range` is for, which is unmapped: for a fault
 * (`fault`), -EBUSY rather than wait for a seal (fxfs_dax_store_begin).
 */
static int fxfs_dax_map_chunk(struct fxfs_dax *dax, struct inode *inode,
			      struct fxfs_dax_range *range, bool fault)
{
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = min_t(loff_t, start + ((loff_t)range->pages << PAGE_SHIFT),
			   round_up(i_size_read(inode), PAGE_SIZE));
	struct fxfs_dax_extent *extents;
	ssize_t count;
	long gate;
	u64 seals;
	int err;

	if (end <= start)
		return 0;
	extents = fxfs_dax_alloc_extents();
	if (!extents)
		return -ENOMEM;
	for (;;) {
		/*
		 * Extents open as Rust finds them are still once the gate is entered, unless a seal
		 * was in progress then, or began since.
		 */
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(dax->fs, inode->i_ino, start, end - start,
						   extents, FXFS_DAX_PAGES));
		if (count < 0) {
			err = count;
			break;
		}
		/* Sealed extents map read-only, whatever seals meanwhile, outside the gate. */
		if (!fxfs_dax_any_open(extents, count)) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			break;
		}
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			break;
		}
		if (gate == seals) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			fxfs_device_store_end(dax->dev);
			break;
		}
		fxfs_device_store_end(dax->dev);
	}
	kvfree(extents);
	return err;
}

/* Wakes those waiting for a range to be freed, or for its last user to be done. */
static void fxfs_dax_released(struct fxfs_dax *dax)
{
	atomic_long_inc(&dax->released);
	wake_up_all(&dax->wait);
}

/* Waits up to `ms` milliseconds for a range to be freed, or its last user done, since `seen`. */
static long fxfs_dax_wait_released(struct fxfs_dax *dax, long seen, unsigned int ms)
{
	return wait_event_killable_timeout(dax->wait, atomic_long_read(&dax->released) != seen,
					   msecs_to_jiffies(ms));
}

/*
 * `pages` pages of the window's free space: their offset in the window, or -ENOSPC if it has no
 * room. A whole chunk's range is at a chunk's alignment, if `aligned`.
 */
static long fxfs_dax_take_window(struct fxfs_dax *dax, unsigned long pages, bool aligned)
{
	size_t size = pages << PAGE_SHIFT;
	unsigned long addr;

	if (aligned && pages == FXFS_DAX_PAGES)
		addr = gen_pool_alloc_algo(dax->pool, size, gen_pool_first_fit_order_align, NULL);
	else
		addr = gen_pool_alloc(dax->pool, size);
	return addr ? addr - FXFS_DAX_POOL_BASE : -ENOSPC;
}

/* Frees `pages` pages of the window at `window_offset`, which nothing maps. */
static void fxfs_dax_give_window(struct fxfs_dax *dax, u64 window_offset, unsigned long pages)
{
	if (pages)
		gen_pool_free(dax->pool, window_offset + FXFS_DAX_POOL_BASE, pages << PAGE_SHIFT);
}

/*
 * A new range of `pages` pages at `window_offset`, unmapped and on no list: of its own, or a slice
 * of `region`, whose slice it takes.
 */
static struct fxfs_dax_range *fxfs_dax_new_range(u64 window_offset, unsigned long pages,
						 struct fxfs_dax_region *region)
{
	struct fxfs_dax_range *range;

	range = kzalloc(sizeof(*range), GFP_NOFS);
	if (!range)
		return ERR_PTR(-ENOMEM);
	range->mapped = bitmap_zalloc(pages, GFP_NOFS);
	range->writable = bitmap_zalloc(pages, GFP_NOFS);
	range->relocated = bitmap_zalloc(pages, GFP_NOFS);
	if (!range->mapped || !range->writable || !range->relocated) {
		bitmap_free(range->mapped);
		bitmap_free(range->writable);
		bitmap_free(range->relocated);
		kfree(range);
		return ERR_PTR(-ENOMEM);
	}
	range->window_offset = window_offset;
	range->region = region;
	range->pages = pages;
	INIT_LIST_HEAD(&range->list);
	return range;
}

static u64 fxfs_dax_region_key(u64 branch, u64 sequence)
{
	return sequence ^ (branch * GOLDEN_RATIO_64);
}

/* The region mapping the extent {branch, sequence}, if there is one. With the regions lock held. */
static struct fxfs_dax_region *fxfs_dax_find_region(struct fxfs_dax *dax, u64 branch,
						    u64 sequence)
{
	struct fxfs_dax_region *region;

	hash_for_each_possible(dax->regions, region, node, fxfs_dax_region_key(branch, sequence)) {
		if (region->branch == branch && region->sequence == sequence)
			return region;
	}
	return NULL;
}

/*
 * Drops a reference to `region`: with its last, the region is unmapped, and its space free again.
 */
static void fxfs_dax_put_region(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	int err;

	spin_lock(&dax->regions_lock);
	if (--region->refs) {
		spin_unlock(&dax->regions_lock);
		return;
	}
	hash_del(&region->node);
	list_del(&region->writable_list);
	spin_unlock(&dax->regions_lock);
	if (region->mapped) {
		err = fxfs_device_unmap(dax->dev, region->window_offset,
					(u64)region->mapped << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				region->window_offset, err);
	}
	fxfs_dax_give_window(dax, region->window_offset, region->pages);
	kfree(region);
	fxfs_dax_released(dax);
}

/* Frees what `range` holds in memory, and it. */
static void fxfs_dax_free_range_memory(struct fxfs_dax_range *range)
{
	bitmap_free(range->mapped);
	bitmap_free(range->writable);
	bitmap_free(range->relocated);
	kfree(range->moved);
	kfree(range->regions);
	kfree(range);
}

static void fxfs_dax_free_range_rcu(struct rcu_head *rcu)
{
	fxfs_dax_free_range_memory(container_of(rcu, struct fxfs_dax_range, rcu));
}

/*
 * Frees `range`, which nothing maps, and is in no file's ranges: its space in the window is free
 * again, or its slice, and the regions its pages moved to are its no more.
 */
static void fxfs_dax_free_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	unsigned int i;

	spin_lock(&dax->lock);
	list_del(&range->list);
	spin_unlock(&dax->lock);
	if (range->region)
		fxfs_dax_put_region(dax, range->region);
	else
		fxfs_dax_give_window(dax, range->window_offset, range->pages);
	for (i = 0; i < range->nr_regions; i++)
		fxfs_dax_put_region(dax, range->regions[i]);
	call_rcu(&range->rcu, fxfs_dax_free_range_rcu);
	fxfs_dax_released(dax);
}

/*
 * Unmaps `range` and frees it, with its file's DAX semaphore held for writing, once nothing maps
 * its pages: its file's inode's mappings of them are gone (fxfs_dax_release), if it has one. A
 * slice's pages are its region's, which go with its last slice.
 */
static void fxfs_dax_unmap_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	int err;

	xa_erase(&range->file->ranges, range->chunk);
	if (!range->region && !bitmap_empty(range->mapped, range->pages)) {
		err = fxfs_device_unmap(dax->dev, range->window_offset,
					(u64)range->pages << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
	}
	fxfs_dax_free_range(dax, range);
}

/* Drops a reference to `file`, freeing it once it has none. */
static void fxfs_dax_put_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	if (!refcount_dec_and_test(&file->refs))
		return;
	mutex_lock(&dax->writable_lock);
	list_del_init(&file->writable);
	mutex_unlock(&dax->writable_lock);
	xa_destroy(&file->ranges);
	kfree(file);
}

/*
 * Forgets `file` if no inode is read in for it and it has no ranges, with the files lock held:
 * returns whether it did, when the caller drops the filesystem's reference to it, once it has
 * unlocked the files lock (fxfs_dax_put_file takes the writable lock, which the sealer takes
 * before it).
 */
static bool fxfs_dax_forget_file(struct fxfs_dax *dax, struct fxfs_dax_file *file)
{
	lockdep_assert_held(&dax->files_lock);
	if (file->inode || !xa_empty(&file->ranges) || xa_load(&dax->files, file->ino) != file)
		return false;
	xa_erase(&dax->files, file->ino);
	return true;
}

/*
 * Removes the inode's mappings of the chunk `range` maps, and the DAX entries for them, unless
 * a page is in use (pinned): with the inode's invalidate_lock held for writing.
 */
static bool fxfs_dax_release(struct inode *inode, struct fxfs_dax_range *range)
{
	struct address_space *mapping = inode->i_mapping;
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = start + ((loff_t)range->pages << PAGE_SHIFT) - 1;

	if (dax_layout_busy_page_range(mapping, start, end))
		return false;
	/* Dirty entries go only once written back, which only cleans them. */
	if (filemap_fdatawrite_range(mapping, start, end))
		return false;
	return !invalidate_inode_pages2_range(mapping, start >> PAGE_SHIFT, end >> PAGE_SHIFT);
}

/*
 * Reclaims `file`'s range of chunk `chunk`, if no iomap uses it and its file's locks are free,
 * and returns whether it did. A file with no inode has nothing mapping its ranges but the window.
 */
static bool fxfs_dax_reclaim_range(struct fxfs_dax *dax, struct fxfs_dax_file *file,
				   pgoff_t chunk)
{
	struct fxfs_dax_range *range;
	struct inode *inode = NULL;
	bool reclaimed = false, forgotten = false;

	mutex_lock(&dax->files_lock);
	if (file->inode)
		inode = igrab(file->inode);
	mutex_unlock(&dax->files_lock);

	if (inode) {
		if (down_write_trylock(&inode->i_mapping->invalidate_lock)) {
			if (down_write_trylock(&file->sem)) {
				range = xa_load(&file->ranges, chunk);
				if (range && !atomic_read(&range->users) &&
				    fxfs_dax_release(inode, range)) {
					fxfs_dax_unmap_range(dax, range);
					reclaimed = true;
				}
				up_write(&file->sem);
			}
			filemap_invalidate_unlock(inode->i_mapping);
		}
		iput(inode);
		return reclaimed;
	}
	if (!down_write_trylock(&file->sem))
		return false;
	mutex_lock(&dax->files_lock);
	/* An inode read in since may map the range: it is reclaimed with it next time. */
	if (!file->inode) {
		range = xa_load(&file->ranges, chunk);
		if (range && !atomic_read(&range->users)) {
			fxfs_dax_unmap_range(dax, range);
			reclaimed = true;
		}
		forgotten = fxfs_dax_forget_file(dax, file);
	}
	mutex_unlock(&dax->files_lock);
	up_write(&file->sem);
	if (forgotten)
		fxfs_dax_put_file(dax, file);
	return reclaimed;
}

/* Reclaims the least recently used range no iomap uses, and returns whether it did. */
static bool fxfs_dax_reclaim(struct fxfs_dax *dax)
{
	struct fxfs_dax_file *file = NULL;
	struct fxfs_dax_range *range;
	pgoff_t chunk = 0;
	bool reclaimed;

	spin_lock(&dax->lock);
	list_for_each_entry(range, &dax->used, list) {
		if (atomic_read(&range->users))
			continue;
		/* Tried once: the next try starts with another. */
		list_move_tail(&range->list, &dax->used);
		file = range->file;
		/* Kept while it is tried: a range on the list keeps its file. */
		refcount_inc(&file->refs);
		chunk = range->chunk;
		break;
	}
	spin_unlock(&dax->lock);
	if (!file)
		return false;
	reclaimed = fxfs_dax_reclaim_range(dax, file, chunk);
	fxfs_dax_put_file(dax, file);
	return reclaimed;
}

/*
 * `pages` pages of the window, as fxfs_dax_take_window, reclaimed if it has no room: their offset.
 * Waits for room, unless `nowait` asks, as a fault does, which can't: -EAGAIN then.
 */
static long fxfs_dax_alloc_window(struct fxfs_dax *dax, unsigned long pages, bool aligned,
				  bool nowait)
{
	long window_offset, seen;

	for (;;) {
		seen = atomic_long_read(&dax->released);
		window_offset = fxfs_dax_take_window(dax, pages, aligned);
		if (window_offset >= 0)
			return window_offset;
		if (fxfs_dax_reclaim(dax))
			continue;
		if (nowait)
			return -EAGAIN;
		/* Ranges in use, or whose inodes are locked, free up soon. */
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return -EINTR;
	}
}

/* A new range of its own of `pages` pages, as fxfs_dax_alloc_window finds room for it. */
static struct fxfs_dax_range *fxfs_dax_alloc_range(struct fxfs_dax *dax, unsigned long pages,
						   bool fault)
{
	long window_offset = fxfs_dax_alloc_window(dax, pages, true, fault);
	struct fxfs_dax_range *range;

	if (window_offset < 0)
		return ERR_PTR(window_offset);
	range = fxfs_dax_new_range(window_offset, pages, NULL);
	if (IS_ERR(range))
		fxfs_dax_give_window(dax, window_offset, pages);
	return range;
}

/*
 * The region mapping extent `e`'s, made, unmapped, if there is none: with a reference more, the
 * caller's. -EAGAIN if the window has no room for it now, reclaiming ranges for it if `reclaim`
 * says: the caller holds a file's DAX semaphore, and can't wait for room.
 */
static struct fxfs_dax_region *fxfs_dax_get_region(struct fxfs_dax *dax,
						   const struct fxfs_dax_extent *e, bool reclaim)
{
	unsigned long pages = DIV_ROUND_UP(e->extent_size, PAGE_SIZE);
	struct fxfs_dax_region *region, *new;
	long window_offset;

	spin_lock(&dax->regions_lock);
	region = fxfs_dax_find_region(dax, e->branch, e->sequence);
	if (region)
		region->refs++;
	spin_unlock(&dax->regions_lock);
	if (region)
		return region;

	new = kzalloc(sizeof(*new), GFP_NOFS);
	if (!new)
		return ERR_PTR(-ENOMEM);
	/* A region maps an extent at any offset: its space needs no alignment. */
	window_offset = reclaim ? fxfs_dax_alloc_window(dax, pages, false, true) :
				  fxfs_dax_take_window(dax, pages, false);
	if (window_offset < 0) {
		kfree(new);
		return ERR_PTR(window_offset);
	}
	new->branch = e->branch;
	new->sequence = e->sequence;
	new->window_offset = window_offset;
	new->pages = pages;
	new->refs = 1;
	mutex_init(&new->lock);
	INIT_LIST_HEAD(&new->writable_list);
	new->dax = dax;
	INIT_WORK(&new->grow, fxfs_dax_grow);
	atomic_set(&new->growing, 0);
	atomic_long_set(&new->reached, 0);
	spin_lock(&dax->regions_lock);
	/* Another may have made it meanwhile. */
	region = fxfs_dax_find_region(dax, e->branch, e->sequence);
	if (region)
		region->refs++;
	else
		hash_add(dax->regions, &new->node, fxfs_dax_region_key(e->branch, e->sequence));
	spin_unlock(&dax->regions_lock);
	if (!region)
		return new;
	fxfs_dax_give_window(dax, window_offset, pages);
	kfree(new);
	return region;
}

/*
 * Makes `region`, of an open extent, writable, unless it is mapped already. In the gate, where only
 * writable regions are mapped further.
 */
static void fxfs_dax_region_open(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	spin_lock(&dax->regions_lock);
	if (!region->writable && !READ_ONCE(region->mapped)) {
		region->writable = true;
		list_add_tail(&region->writable_list, &dax->writable_regions);
	}
	spin_unlock(&dax->regions_lock);
}

/*
 * Maps `region`'s extent as far as page `end` at least, in the gate, where the device has sealed
 * the extents the filesystem has: read-only, all of it if it can; or, while the extent is open,
 * writably, and further, doubling what is mapped by up to FXFS_DAX_REGION_STEP pages, from
 * FXFS_DAX_REGION_FIRST, so that the next blocks it hands out need no MAP. -EXDEV if the region's
 * space doesn't reach so far: sealed, it shrinks to what it mapped.
 */
static int fxfs_dax_map_to(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end)
{
	unsigned long from, to;
	int err = 0;

	mutex_lock(&region->lock);
	from = region->mapped;
	if (end <= from)
		goto out;
	if (end > region->pages) {
		err = -EXDEV;
		goto out;
	}
	to = region->pages;
	if (region->writable)
		to = min(to, max(end, from + clamp(from, FXFS_DAX_REGION_FIRST, FXFS_DAX_REGION_STEP)));
	for (;;) {
		err = fxfs_device_map(dax->dev, region->branch, region->sequence,
				      (u64)from << PAGE_SHIFT, (u64)(to - from) << PAGE_SHIFT,
				      region->window_offset + ((u64)from << PAGE_SHIFT),
				      region->writable);
		/* A sealed extent may end before what was allocated of it: past what was written. */
		if (!err || region->writable || to == end)
			break;
		to = end;
	}
	if (err && err != -ENOENT)
		pr_warn("fxfs: mapping pages %lu to %lu of extent %llx:%llu, %s, at %llu of the DAX window failed: %d\n",
			from, to, region->branch, region->sequence,
			region->writable ? "writably" : "read-only", region->window_offset, err);
	else
		WRITE_ONCE(region->mapped, to);
out:
	mutex_unlock(&region->lock);
	return err;
}

/*
 * Has `region`'s writable extent mapped a step further in the background, unless it is mapped
 * far enough ahead of the writes already: once they reach the second half of what it maps, the
 * next step, so that they rarely wait for a MAP (a MAP of a step takes most of a millisecond).
 */
static void fxfs_dax_grow_ahead(struct fxfs_dax *dax, struct fxfs_dax_region *region)
{
	unsigned long mapped = READ_ONCE(region->mapped);
	unsigned long reached = atomic_long_read(&region->reached);

	if (!READ_ONCE(region->writable) || mapped >= region->pages ||
	    reached + clamp(mapped, FXFS_DAX_REGION_FIRST, FXFS_DAX_REGION_STEP) / 2 <= mapped)
		return;
	if (atomic_cmpxchg(&region->growing, 0, 1))
		return;
	spin_lock(&dax->regions_lock);
	region->refs++;
	spin_unlock(&dax->regions_lock);
	atomic_inc(&dax->growing);
	queue_work(dax->grow_wq, &region->grow);
}

/*
 * fxdax: maps `region`'s writable extent one step further, in the gate (unless it is sealed by
 * then), and again if the writes went on meanwhile; and drops the reference its queuing took.
 */
static void fxfs_dax_grow(struct work_struct *work)
{
	struct fxfs_dax_region *region = container_of(work, struct fxfs_dax_region, grow);
	struct fxfs_dax *dax = region->dax;
	unsigned long mapped;

	fxfs_device_store_begin(dax->dev);
	/* In the gate, no seal makes it read-only, or shrinks it. */
	mapped = READ_ONCE(region->mapped);
	if (READ_ONCE(region->writable) && mapped < region->pages)
		fxfs_dax_map_to(dax, region, mapped + 1);
	fxfs_device_store_end(dax->dev);
	atomic_set(&region->growing, 0);
	fxfs_dax_grow_ahead(dax, region);
	fxfs_dax_put_region(dax, region);
	atomic_dec(&dax->growing);
}

/*
 * Maps `region`'s extent as far as page `end` at least (fxfs_dax_map_to), in the gate; and a
 * writable region's further, in the background, ahead of the writes (fxfs_dax_grow_ahead).
 */
static int fxfs_dax_reach(struct fxfs_dax *dax, struct fxfs_dax_region *region, unsigned long end)
{
	unsigned long reached;
	int err;

	err = fxfs_dax_map_to(dax, region, end);
	if (err || !READ_ONCE(region->writable))
		return err;
	reached = atomic_long_read(&region->reached);
	while (reached < end && !atomic_long_try_cmpxchg(&region->reached, &reached, end))
		;
	fxfs_dax_grow_ahead(dax, region);
	return 0;
}

/*
 * Keeps a reference to `region`, whose reference the caller passes, in `range`'s regions, unless it
 * has one: the caller's goes then.
 */
static int fxfs_dax_keep_region(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				struct fxfs_dax_region *region)
{
	struct fxfs_dax_region **regions;
	unsigned int i;

	for (i = 0; i < range->nr_regions; i++) {
		if (range->regions[i] == region) {
			fxfs_dax_put_region(dax, region);
			return 0;
		}
	}
	/* Room for 4, then twice as many as it has as it fills. */
	if (!range->nr_regions || (range->nr_regions >= 4 && is_power_of_2(range->nr_regions))) {
		regions = krealloc_array(range->regions, max(4U, 2 * range->nr_regions),
					 sizeof(*regions), GFP_NOFS);
		if (!regions) {
			fxfs_dax_put_region(dax, region);
			return -ENOMEM;
		}
		range->regions = regions;
	}
	range->regions[range->nr_regions++] = region;
	return 0;
}

/*
 * fxdax: maps the piece `e`, pages `page` on of the chunk the range of its own `range` maps, of an
 * extent no larger than a chunk, where the extent's region maps it, rather than in the range's own
 * space (the region made if there is none, and mapped as far as the piece, writably if it is
 * open): so that the window maps the extent once, and blocks moved to the staging extent take no
 * MAP. -ENOSPC if the window has no room for the region now, -EXDEV if the region can't map the
 * piece writably, as it is sealed. In the gate if `e` is open, with the file's DAX semaphore held
 * for writing.
 */
static int fxfs_dax_move(struct fxfs_dax *dax, struct fxfs_dax_range *range,
			 const struct fxfs_dax_extent *e, unsigned long page, unsigned long pages)
{
	unsigned long first = e->extent_offset >> PAGE_SHIFT;
	struct fxfs_dax_region *region;
	int err;

	if (!range->moved) {
		range->moved = kcalloc(range->pages, sizeof(*range->moved), GFP_NOFS);
		if (!range->moved)
			return -ENOMEM;
	}
	/* Not reclaiming other ranges for it, in the gate: the range's own space maps it then. */
	region = fxfs_dax_get_region(dax, e, false);
	if (IS_ERR(region))
		return PTR_ERR(region);
	if (e->open) {
		fxfs_dax_region_open(dax, region);
		if (!region->writable) {
			fxfs_dax_put_region(dax, region);
			return -EXDEV;
		}
	}
	err = fxfs_dax_reach(dax, region, first + pages);
	if (err) {
		fxfs_dax_put_region(dax, region);
		return err;
	}
	err = fxfs_dax_keep_region(dax, range, region);
	if (err)
		return err;
	fxfs_dax_relocate(range, page, pages, region, first);
	return 0;
}

/*
 * Makes `inode`'s chunk `chunk` a slice of its extent's region, if its data in the file's `size`
 * bytes is one piece of an extent the window maps whole, and returns it, kept, in its file's
 * ranges: writable if the extent is open. NULL if it isn't such a piece, or the window has no room
 * for its region now; for a fault (`fault`), -EBUSY rather than wait for a seal. With its file's
 * DAX semaphore held for writing.
 */
static struct fxfs_dax_range *fxfs_dax_new_slice(struct fxfs_dax *dax, struct inode *inode,
						 pgoff_t chunk, loff_t size, bool fault)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	loff_t start = (loff_t)chunk << FXFS_DAX_SHIFT;
	loff_t end = min_t(loff_t, start + FXFS_DAX_SIZE, round_up(size, PAGE_SIZE));
	struct fxfs_dax_region *region;
	struct fxfs_dax_range *range;
	struct fxfs_dax_extent e;
	unsigned long first, pages;
	ssize_t count;
	long gate;
	u64 seals;
	int err;

	if (end <= start)
		return NULL;
	pages = (end - start) >> PAGE_SHIFT;
	for (;;) {
		seals = fxfs_device_seals(dax->dev);
		count = fxfs_nofs(fxfs_dax_extents(dax->fs, inode->i_ino, start, end - start, &e, 1));
		/* Holes, or more than one piece. */
		if (count == 0 || count == -E2BIG)
			return NULL;
		if (count < 0)
			return ERR_PTR(count);
		if (e.offset != start || e.len != end - start || !e.extent_size ||
		    e.extent_size > FXFS_DAX_SIZE || !PAGE_ALIGNED(e.extent_offset))
			return NULL;
		region = fxfs_dax_get_region(dax, &e, true);
		if (IS_ERR(region))
			return PTR_ERR(region) == -EAGAIN ? NULL : ERR_CAST(region);
		first = e.extent_offset >> PAGE_SHIFT;
		/* A sealed extent maps read-only, whatever seals meanwhile, outside the gate. */
		if (!e.open)
			break;
		/* As in fxfs_dax_map_chunk: open still, unless a seal was in progress, or began since. */
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			fxfs_dax_put_region(dax, region);
			return ERR_PTR(gate);
		}
		if (gate == seals) {
			fxfs_dax_region_open(dax, region);
			break;
		}
		fxfs_device_store_end(dax->dev);
		fxfs_dax_put_region(dax, region);
	}
	err = fxfs_dax_reach(dax, region, first + pages);
	if (err)
		goto out;
	range = fxfs_dax_new_range(region->window_offset + ((u64)first << PAGE_SHIFT), pages,
				   region);
	if (IS_ERR(range)) {
		err = PTR_ERR(range);
		goto out;
	}
	range->file = file;
	range->chunk = chunk;
	atomic_set(&range->users, 1);
	bitmap_set(range->mapped, 0, pages);
	err = xa_err(xa_store(&file->ranges, chunk, range, GFP_NOFS));
	if (err) {
		/* Its slice goes with it. */
		region = NULL;
		fxfs_dax_free_range(dax, range);
		goto out;
	}
	if (e.open && region->writable) {
		bitmap_set(range->writable, 0, pages);
		fxfs_dax_add_writable(dax, file);
	}
	spin_lock(&dax->lock);
	list_add_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
	if (e.open)
		fxfs_device_store_end(dax->dev);
	return range;
out:
	if (e.open)
		fxfs_device_store_end(dax->dev);
	if (region)
		fxfs_dax_put_region(dax, region);
	/*
	 * Its region can't reach it, or its extent was never written: a range of its own maps it,
	 * or the holes it is.
	 */
	return err == -EXDEV || err == -ENOENT ? NULL : ERR_PTR(err);
}

/*
 * Makes room for a fault that found none, holding the lock it held: reclaims a range, which it can
 * now, if one of its own inode's, or waits a moment for one released since `seen`.
 */
static void fxfs_dax_make_room(struct fxfs_dax *dax, long seen)
{
	if (!fxfs_dax_reclaim(dax))
		fxfs_dax_wait_released(dax, seen, 10);
}

/* How many pages the range of chunk `chunk` of a file `size` bytes long takes. */
static unsigned long fxfs_dax_range_pages(pgoff_t chunk, loff_t size)
{
	loff_t start = (loff_t)chunk << FXFS_DAX_SHIFT;

	if (size <= start)
		return 1;
	return min_t(unsigned long, FXFS_DAX_PAGES, DIV_ROUND_UP(size - start, PAGE_SIZE));
}

/*
 * How many pages a range of its own for chunk `chunk` of a file `size` bytes long takes: what the
 * file needs, rounded up to a power of two, so that a file growing a page at a time, as a log
 * appended to does, outgrows its range, and maps its data again, only as often as its size
 * doubles.
 */
static unsigned long fxfs_dax_range_capacity(pgoff_t chunk, loff_t size)
{
	return min_t(unsigned long, FXFS_DAX_PAGES,
		     roundup_pow_of_two(fxfs_dax_range_pages(chunk, size)));
}

/*
 * The range mapping `inode`'s chunk `chunk`, mapped first if it isn't, for a file at least `size`
 * bytes long, kept for an iomap: a slice of its extent's region if it can be, or a range of its
 * own. For a fault (`fault`), -EBUSY rather than wait for a seal (fxfs_dax_store_begin).
 */
static struct fxfs_dax_range *fxfs_dax_get_range(struct inode *inode, pgoff_t chunk, loff_t size,
						 bool fault)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range, *new;
	unsigned long pages;
	int err;

	down_read(&file->sem);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	up_read(&file->sem);
	if (range)
		goto used;

	down_write(&file->sem);
	range = xa_load(&file->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	else
		range = fxfs_dax_new_slice(dax, inode, chunk, max(i_size_read(inode), size), fault);
	up_write(&file->sem);
	if (IS_ERR(range))
		return range;
	if (range)
		goto used;

	pages = fxfs_dax_range_capacity(chunk, max(i_size_read(inode), size));
	new = fxfs_dax_alloc_range(dax, pages, fault);
	if (IS_ERR(new))
		return new;
	down_write(&file->sem);
	/* Another iomap may have mapped it meanwhile. */
	range = xa_load(&file->ranges, chunk);
	if (range) {
		atomic_inc(&range->users);
		up_write(&file->sem);
		fxfs_dax_free_range(dax, new);
		goto used;
	}
	new->file = file;
	new->chunk = chunk;
	atomic_set(&new->users, 1);
	err = xa_err(xa_store(&file->ranges, chunk, new, GFP_NOFS));
	if (!err) {
		spin_lock(&dax->lock);
		list_add_tail(&new->list, &dax->used);
		spin_unlock(&dax->lock);
		err = fxfs_dax_map_chunk(dax, inode, new, fault);
		if (err)
			fxfs_dax_unmap_range(dax, new);
	} else {
		fxfs_dax_free_range(dax, new);
	}
	up_write(&file->sem);
	if (err)
		return ERR_PTR(err);
	range = new;
used:
	spin_lock(&dax->lock);
	list_move_tail(&range->list, &dax->used);
	spin_unlock(&dax->lock);
	return range;
}

static void fxfs_dax_put_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	if (atomic_dec_and_test(&range->users))
		fxfs_dax_released(dax);
}

/*
 * Drops `inode`'s ranges of chunks `first` to `last` too small for the file to be `size` bytes
 * long, or all of them if `size` is negative, once nothing uses their pages: they are mapped again
 * as needed. With the inode's invalidate_lock held for writing.
 */
static int fxfs_dax_drop(struct inode *inode, pgoff_t first, pgoff_t last, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	int err;

	for (;;) {
		long seen = atomic_long_read(&dax->released);
		bool busy = false;

		down_write(&file->sem);
		xa_for_each_range(&file->ranges, chunk, range, first, last) {
			if (size >= 0 && range->pages >= fxfs_dax_range_pages(chunk, size))
				continue;
			if (atomic_read(&range->users) || !fxfs_dax_release(inode, range)) {
				busy = true;
				break;
			}
			fxfs_dax_unmap_range(dax, range);
		}
		up_write(&file->sem);
		if (!busy)
			return 0;
		/* A fault in progress, or a page pinned: wait for it. */
		err = fxfs_dax_break_layouts(inode, (loff_t)first << FXFS_DAX_SHIFT);
		if (err)
			return err;
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return -EINTR;
	}
}

/*
 * Drops the ranges of `inode`'s file too small for it to be `size` bytes long: those of the chunks
 * its end moves past or into, which are mapped again as large as they need. With the inode's lock
 * and invalidate_lock held for writing.
 */
static int fxfs_dax_fit(struct inode *inode, loff_t size)
{
	loff_t old = i_size_read(inode);

	if (size <= old)
		return 0;
	return fxfs_dax_drop(inode, old >> FXFS_DAX_SHIFT, (size - 1) >> FXFS_DAX_SHIFT, size);
}

/*
 * Drops `inode`'s range of the chunk at `pos`, a slice a write can't use (-EXDEV), to be mapped
 * again where its data now is.
 */
static int fxfs_dax_drop_slice(struct inode *inode, loff_t pos)
{
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1);
	filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

/*
 * Makes room for a write of `len` bytes at `pos` of `inode`, whose chunk has no range yet, and
 * makes the file `size` bytes long if it is shorter: so that its range maps where its data is to
 * be, a slice of the region of an extent small files share, if it can be.
 */
static int fxfs_dax_prepare_write(struct inode *inode, loff_t pos, loff_t len, loff_t size,
				  bool fault, struct fxfs_dax_extent *extents)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	ssize_t count = 0;

	down_write(&file->sem);
	if (!xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT)) {
		count = fxfs_nofs(fxfs_dax_map_write(dax->fs, inode->i_ino, pos, len, size, fault,
						     extents, FXFS_DAX_PAGES));
		if (count >= 0 && size > i_size_read(inode))
			i_size_write(inode, size);
	}
	up_write(&file->sem);
	return count < 0 ? count : 0;
}

/* Fills in `iomap` for a hole of `length` bytes. */
static void fxfs_dax_hole(struct iomap *iomap, loff_t length)
{
	iomap->type = IOMAP_HOLE;
	iomap->addr = IOMAP_NULL_ADDR;
	iomap->length = length;
}

/*
 * Maps a read of up to `length` bytes at `pos`: a run of mapped pages in a row in the window, or of
 * holes. A fault's iomap of pages relocated since says they are shared (fxfs_dax_cow).
 */
static int fxfs_dax_iomap_read(struct inode *inode, loff_t pos, loff_t length, unsigned int flags,
			       struct iomap *iomap, struct iomap *srcmap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long page, end_page;
	loff_t start;
	bool mapped;
	u64 at = 0;

	if (pos >= i_size_read(inode)) {
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, 0, flags & IOMAP_FAULT);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	page = (pos - start) >> PAGE_SHIFT;
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(page >= range->pages)) {
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	down_read(&file->sem);
	mapped = test_bit(page, range->mapped);
	end_page = mapped ? find_next_zero_bit(range->mapped, range->pages, page) :
			    find_next_bit(range->mapped, range->pages, page);
	if (mapped) {
		at = fxfs_dax_page_offset(range, page);
		/* As far as the read goes, at most: the run needn't be found further. */
		end_page = min_t(unsigned long, end_page,
				 page + DIV_ROUND_UP(offset_in_page(pos) + length, PAGE_SIZE));
		end_page = page + fxfs_dax_run(range, page, end_page);
	}
	length = min_t(loff_t, length, start + ((loff_t)end_page << PAGE_SHIFT) - pos);
	if (!mapped) {
		up_read(&file->sem);
		fxfs_dax_put_range(dax, range);
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	iomap->type = IOMAP_MAPPED;
	iomap->addr = at + offset_in_page(pos);
	iomap->length = length;
	iomap->private = range;
	/* A read's copy needs no mapping of the pages replaced. */
	if (flags & IOMAP_FAULT)
		fxfs_dax_cow(range, page, end_page - page, iomap, srcmap);
	up_read(&file->sem);
	return 0;
}

/* The guest's memory of the window's page at `window_offset`. */
static void *fxfs_dax_window_page(struct fxfs_dax *dax, u64 window_offset)
{
	void *kaddr;

	if (dax_direct_access(dax->dax_dev, window_offset >> PAGE_SHIFT, 1, DAX_ACCESS, &kaddr,
			      NULL) < 1)
		return NULL;
	return kaddr;
}

/*
 * Maps a write of up to `length` bytes at `pos` writably, making room for it in open extents
 * unless its pages are writable already, and returns in the gate, which fxfs_dax_iomap_end leaves.
 * A zeroing write leaves holes be. A write past the file's end grows it, unless it is a fault's.
 */
static int fxfs_dax_iomap_write(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap, struct iomap *srcmap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_extent *extents = NULL;
	struct fxfs_dax_range *range;
	unsigned long first, pages;
	loff_t start, end, size;
	bool fresh = false;
	/*
	 * A fault's page, which a store may change any part of, as it held it: copied here from its
	 * mapping and back once it moves, rather than by Fxfs through the device, a read and a write.
	 */
	void *held = NULL;
	/* Whether Fxfs moved the page held, leaving it to be copied back. */
	bool moved = false;
	bool fault = flags & IOMAP_FAULT;
	ssize_t count;
	long gate;
	u64 seals;
	int err = 0;

	/* Faults and zeroing don't grow the file. */
	size = flags & (IOMAP_FAULT | IOMAP_ZERO) ? 0 :
	       min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	/* A chunk with no range yet: room for the write first. Zeroing leaves holes be. */
	if (!(flags & IOMAP_ZERO) && !xa_load(&file->ranges, pos >> FXFS_DAX_SHIFT)) {
		extents = fxfs_dax_alloc_extents();
		if (!extents)
			return -ENOMEM;
		err = fxfs_dax_prepare_write(inode, pos,
					     min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE)) - pos,
					     size, fault, extents);
		if (err) {
			kvfree(extents);
			return err;
		}
		/* Its pages were holes, which mappings of the file may map zeros for. */
		fresh = true;
	}
	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, size, fault);
	if (IS_ERR(range)) {
		kvfree(extents);
		return PTR_ERR(range);
	}
	start = fxfs_dax_chunk_start(range);
	end = min_t(loff_t, pos + length, start + ((loff_t)range->pages << PAGE_SHIFT));
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(end <= pos)) {
		kvfree(extents);
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	if (size)
		size = end;
	first = (pos - start) >> PAGE_SHIFT;
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;

	down_write(&file->sem);
	for (;;) {
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			goto out;
		}
		/* Writable pages take the write as they are, unless it grows the file. */
		if (find_next_zero_bit(range->writable, first + pages, first) == first + pages &&
		    size <= i_size_read(inode))
			break;
		if (flags & IOMAP_ZERO && find_next_bit(range->mapped, first + pages, first) ==
						  first + pages) {
			fxfs_device_store_end(dax->dev);
			up_write(&file->sem);
			fxfs_dax_put_range(dax, range);
			fxfs_dax_hole(iomap, end - pos);
			return 0;
		}
		fxfs_device_store_end(dax->dev);

		if (!extents) {
			extents = fxfs_dax_alloc_extents();
			if (!extents) {
				err = -ENOMEM;
				goto out;
			}
		}
		fresh |= find_next_zero_bit(range->mapped, first + pages, first) != first + pages;
		if (fault && pages == 1 && test_bit(first, range->mapped) && !held) {
			void *page = fxfs_dax_window_page(dax, fxfs_dax_page_offset(range, first));

			held = page ? kmalloc(PAGE_SIZE, GFP_NOFS) : NULL;
			if (held)
				memcpy(held, page, PAGE_SIZE);
		}
		/* As in fxfs_dax_map_chunk: unless a seal was in progress, or began since. */
		seals = fxfs_device_seals(dax->dev);
		/* A fault's pages hold what they held: a store may change any part of them. */
		count = fxfs_nofs(fxfs_dax_map_write(dax->fs, inode->i_ino, pos, end - pos, size,
						     fault && !held, extents,
						     FXFS_DAX_PAGES));
		if (count < 0) {
			err = count;
			goto out;
		}
		moved |= held != NULL;
		if (size > i_size_read(inode))
			i_size_write(inode, size);
		gate = fxfs_dax_store_begin(dax, fault);
		if (gate < 0) {
			err = gate;
			goto out;
		}
		if (gate == seals) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			if (err) {
				fxfs_device_store_end(dax->dev);
				goto out;
			}
			break;
		}
		fxfs_device_store_end(dax->dev);
	}
	/* In the gate. The page held, now in its new block, which Fxfs left zeros. */
	if (held) {
		void *page = fxfs_dax_window_page(dax, fxfs_dax_page_offset(range, first));

		if (page)
			memcpy(page, held, PAGE_SIZE);
		kfree(held);
	}
	kvfree(extents);
	iomap->type = IOMAP_MAPPED;
	/* Holes a mapping of the file maps zeros for are gone. */
	iomap->flags |= IOMAP_F_PRIVATE | (fresh ? IOMAP_F_NEW : 0);
	iomap->addr = fxfs_dax_page_offset(range, first) + offset_in_page(pos);
	/* The pages in a row in the window: the next iomap maps those after. */
	iomap->length = min_t(loff_t, end, start + ((loff_t)(first + fxfs_dax_run(range, first,
								  first + pages)) << PAGE_SHIFT)) - pos;
	iomap->private = range;
	fxfs_dax_cow(range, first, DIV_ROUND_UP(offset_in_page(pos) + iomap->length, PAGE_SIZE),
		     iomap, srcmap);
	up_write(&file->sem);
	return 0;
out:
	/*
	 * A fault's page held, whose block Fxfs moved, but which the window didn't map where it is
	 * now: out of a slice's region (-EXDEV), or a seal began (-EBUSY). Fxfs writes it there,
	 * before the semaphore lets anything map the block. What the page holds past the file's end
	 * isn't the file's.
	 */
	if (moved) {
		loff_t page_pos = start + ((loff_t)first << PAGE_SHIFT);
		loff_t len = min_t(loff_t, PAGE_SIZE, i_size_read(inode) - page_pos);
		int write_err = 0;

		if (len > 0)
			write_err = fxfs_nofs(fxfs_write(dax->fs, inode->i_ino, page_pos, held, len));
		if (write_err)
			err = write_err;
	}
	up_write(&file->sem);
	kfree(held);
	kvfree(extents);
	fxfs_dax_put_range(dax, range);
	return err;
}

static int fxfs_dax_iomap_begin(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap, struct iomap *srcmap)
{
	if (WARN_ON_ONCE(flags & IOMAP_REPORT))
		return -EIO;
	iomap->offset = pos;
	iomap->flags = 0;
	iomap->bdev = NULL;
	iomap->dax_dev = fxfs_dax_of(inode)->dax_dev;
	if (flags & (IOMAP_WRITE | IOMAP_ZERO))
		return fxfs_dax_iomap_write(inode, pos, length, flags, iomap, srcmap);
	return fxfs_dax_iomap_read(inode, pos, length, flags, iomap, srcmap);
}

static int fxfs_dax_iomap_end(struct inode *inode, loff_t pos, loff_t length, ssize_t written,
			      unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax *dax = fxfs_dax_of(inode);

	if (iomap->flags & IOMAP_F_PRIVATE)
		fxfs_device_store_end(dax->dev);
	if (iomap->private)
		fxfs_dax_put_range(dax, iomap->private);
	return 0;
}

static const struct iomap_ops fxfs_dax_iomap_ops = {
	.iomap_begin = fxfs_dax_iomap_begin,
	.iomap_end = fxfs_dax_iomap_end,
};

static ssize_t fxfs_dax_read_iter(struct kiocb *iocb, struct iov_iter *to)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	ssize_t ret;

	if (!iov_iter_count(to))
		return 0;
	if (iocb->ki_flags & IOCB_NOWAIT) {
		if (!inode_trylock_shared(inode))
			return -EAGAIN;
	} else {
		inode_lock_shared(inode);
	}
	ret = dax_iomap_rw(iocb, to, &fxfs_dax_iomap_ops);
	inode_unlock_shared(inode);
	file_accessed(iocb->ki_filp);
	return ret;
}

/*
 * Sets the file's size, after a write made it longer than what it wrote: as setattr would,
 * with the inode locked.
 */
static int fxfs_dax_set_size(struct inode *inode, loff_t size)
{
	struct fxfs_setattr set = { .valid = FXFS_SET_SIZE, .size = size };
	int err = fxfs_dax_resize_begin(inode, size);

	if (err)
		return err;
	truncate_setsize(inode, size);
	err = fxfs_nofs(fxfs_setattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
	fxfs_dax_resize_end(inode, size);
	return err;
}

/*
 * Writes through the window, copying with page faults disabled: in the gate, a fault on the
 * user's memory could wait for a seal that waits for the gate. The memory is faulted in first, and
 * again whenever a copy stops short.
 */
static ssize_t fxfs_dax_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	loff_t old_size;
	ssize_t ret, done = 0;
	int err;

	if (iocb->ki_flags & IOCB_NOWAIT)
		return -EAGAIN;
	inode_lock(inode);
	old_size = i_size_read(inode);
	ret = generic_write_checks(iocb, from);
	if (ret <= 0)
		goto out;
	ret = file_remove_privs(iocb->ki_filp);
	if (ret)
		goto out;
	ret = file_update_time(iocb->ki_filp);
	if (ret)
		goto out;
	/* The ranges the file's end moves into, mapped as large as they need. */
	if (iocb->ki_pos + iov_iter_count(from) > old_size) {
		filemap_invalidate_lock(inode->i_mapping);
		ret = fxfs_dax_fit(inode, iocb->ki_pos + iov_iter_count(from));
		filemap_invalidate_unlock(inode->i_mapping);
		if (ret)
			goto out;
	}
	while (iov_iter_count(from)) {
		if (fault_in_iov_iter_readable(from, iov_iter_count(from)) == iov_iter_count(from)) {
			ret = -EFAULT;
			break;
		}
		pagefault_disable();
		ret = dax_iomap_rw(iocb, from, &fxfs_dax_iomap_ops);
		pagefault_enable();
		if (ret > 0) {
			done += ret;
		} else if (ret == -EXDEV) {
			/* A slice the write can't use: dropped, the chunk is mapped again. */
			ret = fxfs_dax_drop_slice(inode, iocb->ki_pos);
			if (ret)
				break;
		} else if (ret != -EFAULT) {
			break;
		}
		/* What faulted in may be gone again: the loop faults it in once more. */
	}
	/* A write that stopped short leaves the file no longer than it wrote. */
	if (i_size_read(inode) > max(old_size, iocb->ki_pos)) {
		err = fxfs_dax_set_size(inode, max(old_size, iocb->ki_pos));
		if (err && !done)
			ret = err;
	}
	if (done)
		ret = done;
out:
	inode_unlock(inode);
	if (ret > 0)
		ret = generic_write_sync(iocb, ret);
	return ret;
}

static vm_fault_t fxfs_dax_do_fault(struct vm_fault *vmf, bool write)
{
	struct inode *inode = file_inode(vmf->vma->vm_file);
	struct super_block *sb = inode->i_sb;
	vm_fault_t ret;
	pfn_t pfn;
	int error;

	if (write) {
		sb_start_pagefault(sb);
		file_update_time(vmf->vma->vm_file);
	}
	for (;;) {
		long seen = atomic_long_read(&fxfs_dax_of(inode)->released);

		error = 0;
		filemap_invalidate_lock_shared(inode->i_mapping);
		ret = dax_iomap_fault(vmf, 0, &pfn, &error, &fxfs_dax_iomap_ops);
		filemap_invalidate_unlock_shared(inode->i_mapping);
		if (!(ret & VM_FAULT_ERROR) ||
		    (error != -EAGAIN && error != -EXDEV && error != -EBUSY))
			break;
		if (error == -EBUSY) {
			/* A seal was in progress: waited for without the page's DAX entry. */
			fxfs_device_wait_unsealed(fxfs_dax_of(inode)->dev);
			continue;
		}
		if (error == -EXDEV) {
			/* A slice can't take the store: it maps the chunk no more. */
			if (fxfs_dax_drop_slice(inode, (loff_t)vmf->pgoff << PAGE_SHIFT))
				break;
			continue;
		}
		/* No range was free, nor could the fault reclaim one, holding the lock it holds. */
		fxfs_dax_make_room(fxfs_dax_of(inode), seen);
	}
	if (write)
		sb_end_pagefault(sb);
	return ret;
}

static vm_fault_t fxfs_dax_fault(struct vm_fault *vmf)
{
	return fxfs_dax_do_fault(vmf, vmf->flags & FAULT_FLAG_WRITE);
}

static vm_fault_t fxfs_dax_mkwrite(struct vm_fault *vmf)
{
	return fxfs_dax_do_fault(vmf, true);
}

/* No huge faults: the sealer write-protects page by page (wp_shared_mapping_range). */
static const struct vm_operations_struct fxfs_dax_vm_ops = {
	.fault = fxfs_dax_fault,
	.page_mkwrite = fxfs_dax_mkwrite,
	.pfn_mkwrite = fxfs_dax_mkwrite,
};

static int fxfs_dax_mmap(struct file *file, struct vm_area_struct *vma)
{
	file_accessed(file);
	vma->vm_ops = &fxfs_dax_vm_ops;
	vm_flags_set(vma, VM_MIXEDMAP);
	return 0;
}

static int fxfs_dax_writepages(struct address_space *mapping, struct writeback_control *wbc)
{
	return dax_writeback_mapping_range(mapping, fxfs_dax_of(mapping->host)->dax_dev, wbc);
}

static const struct address_space_operations fxfs_dax_aops = {
	.writepages = fxfs_dax_writepages,
	.dirty_folio = noop_dirty_folio,
};

static const struct file_operations fxfs_dax_file_operations = {
	.open = generic_file_open,
	.llseek = fxfs_vfs_llseek,
	.read_iter = fxfs_dax_read_iter,
	.write_iter = fxfs_dax_write_iter,
	.mmap = fxfs_dax_mmap,
	.splice_read = copy_splice_read,
	.splice_write = iter_file_splice_write,
	.fsync = fxfs_vfs_fsync,
	.fallocate = fxfs_vfs_fallocate,
};

/*
 * Before a seal: makes user mappings of writable pages read-only, and forgets which pages are
 * writable, and which regions, whose extents grow no more: their space in the window past what
 * they map is free again. With no store in progress.
 */
static void fxfs_dax_seal(void *arg)
{
	struct fxfs_dax *dax = arg;
	struct fxfs_dax_region *region, *next_region;
	struct fxfs_dax_file *file, *next;
	struct fxfs_dax_range *range;
	unsigned long chunk;

	spin_lock(&dax->regions_lock);
	list_for_each_entry_safe(region, next_region, &dax->writable_regions, writable_list) {
		region->writable = false;
		list_del_init(&region->writable_list);
		fxfs_dax_give_window(dax, region->window_offset + ((u64)region->mapped << PAGE_SHIFT),
				     region->pages - region->mapped);
		region->pages = region->mapped;
	}
	spin_unlock(&dax->regions_lock);

	mutex_lock(&dax->writable_lock);
	list_for_each_entry_safe(file, next, &dax->writable, writable) {
		/* A file with no inode has nothing mapping its pages but the window. */
		mutex_lock(&dax->files_lock);
		if (file->inode)
			wp_shared_mapping_range(file->inode->i_mapping, 0, ULONG_MAX);
		mutex_unlock(&dax->files_lock);
		/* Ranges freed meanwhile are freed after a grace period (fxfs_dax_free_range). */
		rcu_read_lock();
		xa_for_each(&file->ranges, chunk, range)
			bitmap_zero(range->writable, range->pages);
		rcu_read_unlock();
		list_del_init(&file->writable);
	}
	mutex_unlock(&dax->writable_lock);
}

struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs)
{
	struct fxfs_device *dev = fxfs_dax_device(fs);
	struct dax_device *dax_dev;
	struct fxfs_dax *dax;
	u64 window_len;

	dax_dev = fxfs_device_dax(dev, &window_len);
	if (!dax_dev || window_len < FXFS_DAX_SIZE)
		return NULL;
	dax = kzalloc(sizeof(*dax), GFP_KERNEL);
	if (!dax)
		return ERR_PTR(-ENOMEM);
	dax->pool = gen_pool_create(PAGE_SHIFT, -1);
	if (!dax->pool || gen_pool_add(dax->pool, FXFS_DAX_POOL_BASE, window_len, -1)) {
		if (dax->pool)
			gen_pool_destroy(dax->pool);
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	dax->sb = sb;
	dax->fs = fs;
	dax->dev = dev;
	dax->dax_dev = dax_dev;
	xa_init(&dax->files);
	mutex_init(&dax->files_lock);
	spin_lock_init(&dax->lock);
	INIT_LIST_HEAD(&dax->used);
	init_waitqueue_head(&dax->wait);
	atomic_long_set(&dax->released, 0);
	mutex_init(&dax->writable_lock);
	INIT_LIST_HEAD(&dax->writable);
	hash_init(dax->regions);
	spin_lock_init(&dax->regions_lock);
	INIT_LIST_HEAD(&dax->writable_regions);
	atomic_set(&dax->growing, 0);
	dax->grow_wq = alloc_workqueue("fxfs_dax_grow", WQ_UNBOUND | WQ_MEM_RECLAIM, 0);
	if (!dax->grow_wq) {
		gen_pool_destroy(dax->pool);
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	fxfs_device_set_sealer(dev, fxfs_dax_seal, dax);
	return dax;
}

/*
 * Waits for the regions growing in the background to be as far ahead of the writes so far as
 * they grow: before a sync seals them. A few rounds at most, as writes may go on meanwhile.
 */
void fxfs_dax_settle(struct fxfs_dax *dax)
{
	int round;

	for (round = 0; round < 8 && atomic_read(&dax->growing); round++)
		flush_workqueue(dax->grow_wq);
}

/*
 * Waits for the regions growing in the background, once nothing writes any more: before the
 * filesystem unmounts, while its device takes requests.
 */
void fxfs_dax_quiesce(struct fxfs_dax *dax)
{
	drain_workqueue(dax->grow_wq);
}

/*
 * Frees `dax` once the filesystem is unmounted, its inodes evicted: the window's ranges are
 * unmapped as the device goes.
 */
void fxfs_dax_destroy(struct fxfs_dax *dax)
{
	struct fxfs_dax_range *range, *next;
	struct fxfs_dax_region *region;
	struct fxfs_dax_file *file;
	struct hlist_node *tmp;
	unsigned long ino;
	int bucket;

	list_for_each_entry_safe(range, next, &dax->used, list) {
		xa_erase(&range->file->ranges, range->chunk);
		list_del(&range->list);
		if (!range->region)
			fxfs_dax_give_window(dax, range->window_offset, range->pages);
		/* Its regions go below. */
		fxfs_dax_free_range_memory(range);
	}
	hash_for_each_safe(dax->regions, bucket, tmp, region, node) {
		hash_del(&region->node);
		fxfs_dax_give_window(dax, region->window_offset, region->pages);
		kfree(region);
	}
	xa_for_each(&dax->files, ino, file) {
		xa_erase(&dax->files, ino);
		fxfs_dax_put_file(dax, file);
	}
	xa_destroy(&dax->files);
	destroy_workqueue(dax->grow_wq);
	gen_pool_destroy(dax->pool);
	kfree(dax);
}

void fxfs_dax_inode_init(struct fxfs_dax_inode *di)
{
	di->file = NULL;
}

/*
 * The file `ino`, whose ranges outlive its inodes, made if it has none, with `inode` read in for
 * it; NULL without the memory for it.
 */
static struct fxfs_dax_file *fxfs_dax_attach(struct fxfs_dax *dax, struct inode *inode)
{
	struct fxfs_dax_file *file;

	mutex_lock(&dax->files_lock);
	file = xa_load(&dax->files, inode->i_ino);
	if (!file) {
		file = kzalloc(sizeof(*file), GFP_NOFS);
		if (!file)
			goto out;
		file->ino = inode->i_ino;
		refcount_set(&file->refs, 1);
		init_rwsem(&file->sem);
		xa_init(&file->ranges);
		INIT_LIST_HEAD(&file->writable);
		if (xa_err(xa_store(&dax->files, inode->i_ino, file, GFP_NOFS))) {
			kfree(file);
			file = NULL;
			goto out;
		}
	}
	WARN_ON_ONCE(file->inode);
	file->inode = inode;
out:
	mutex_unlock(&dax->files_lock);
	return file;
}

void fxfs_dax_set_inode(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_attach(fxfs_dax_of(inode), inode);

	/* Without its file, it stays in the page cache. */
	if (!file)
		return;
	fxfs_vfs_inode(inode)->dax.file = file;
	inode->i_flags |= S_DAX;
	inode->i_fop = &fxfs_dax_file_operations;
	inode->i_mapping->a_ops = &fxfs_dax_aops;
}

/*
 * Detaches `inode`, which the kernel evicts, from its file, whose ranges stay mapped for the next
 * inode read in for it: unless no name links it, when they go.
 */
void fxfs_dax_evict(struct inode *inode)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	bool forgotten;

	if (!dax || !IS_DAX(inode) || !file)
		return;
	if (!inode->i_nlink) {
		down_write(&file->sem);
		xa_for_each(&file->ranges, chunk, range)
			fxfs_dax_unmap_range(dax, range);
		up_write(&file->sem);
	}
	mutex_lock(&dax->files_lock);
	file->inode = NULL;
	forgotten = fxfs_dax_forget_file(dax, file);
	mutex_unlock(&dax->files_lock);
	if (forgotten)
		fxfs_dax_put_file(dax, file);
	fxfs_vfs_inode(inode)->dax.file = NULL;
}

/* Waits for the pages of `inode` from `start` on to be unused, as fuse_dax_break_layouts. */
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start)
{
	struct page *page;
	int err;

	while ((page = dax_layout_busy_page_range(inode->i_mapping, start, LLONG_MAX))) {
		err = ___wait_var_event(&page->_refcount, atomic_read(&page->_refcount) == 1,
					TASK_INTERRUPTIBLE, 0, 0,
					filemap_invalidate_unlock(inode->i_mapping);
					schedule();
					filemap_invalidate_lock(inode->i_mapping));
		if (err)
			return err;
	}
	return 0;
}

/*
 * Zeroes the `len` bytes of `inode` at `pos`, in one page, through the window: dropping a slice
 * that can't take them where it is, to be mapped again. With the invalidate_lock held for writing.
 */
/*
 * Writes back `inode`'s DAX entries of the pages `len` bytes at `pos` touch, cleaning them, before
 * those pages are zeroed: zeroing may move a page (fxfs_dax_cow), and dax_zero_iter invalidates
 * the entries of a moved page without cleaning them first, which leaves a dirty one, of the page
 * it moved from. With the invalidate_lock held for writing, which keeps faults from dirtying them
 * again.
 */
static int fxfs_dax_clean(struct inode *inode, loff_t pos, loff_t len)
{
	return filemap_write_and_wait_range(inode->i_mapping, round_down(pos, PAGE_SIZE),
					    round_up(pos + len, PAGE_SIZE) - 1);
}

static int fxfs_dax_zero_part(struct inode *inode, loff_t pos, loff_t len)
{
	bool zeroed;
	int err = fxfs_dax_clean(inode, pos, len);

	if (err)
		return err;
	for (;;) {
		err = dax_zero_range(inode, pos, len, &zeroed, &fxfs_dax_iomap_ops);
		if (err != -EXDEV)
			return err;
		err = fxfs_dax_drop(inode, pos >> FXFS_DAX_SHIFT, pos >> FXFS_DAX_SHIFT, -1);
		if (err)
			return err;
	}
}

int fxfs_dax_punch_begin(struct inode *inode, loff_t start, loff_t end)
{
	loff_t first = round_up(start, PAGE_SIZE), last = round_down(end, PAGE_SIZE);
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	err = fxfs_dax_break_layouts(inode, round_down(start, PAGE_SIZE));
	if (!err && first > last) {
		err = fxfs_dax_zero_part(inode, start, end - start);
	} else {
		if (!err && start < first)
			err = fxfs_dax_zero_part(inode, start, first - start);
		if (!err && last < end)
			err = fxfs_dax_zero_part(inode, last, end - last);
	}
	/* The chunks of the whole pages freed, mapped again from where their data is, if anywhere. */
	if (!err && first < last)
		err = fxfs_dax_drop(inode, first >> FXFS_DAX_SHIFT, (last - 1) >> FXFS_DAX_SHIFT, -1);
	if (err)
		filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

void fxfs_dax_punch_end(struct inode *inode)
{
	filemap_invalidate_unlock(inode->i_mapping);
}

int fxfs_dax_resize_begin(struct inode *inode, loff_t size)
{
	bool zeroed;
	int err;

	filemap_invalidate_lock(inode->i_mapping);
	if (size >= i_size_read(inode)) {
		/* The ranges the file's end moves into, mapped as large as they need. */
		err = fxfs_dax_fit(inode, size);
		if (err)
			filemap_invalidate_unlock(inode->i_mapping);
		return err;
	}
	err = fxfs_dax_break_layouts(inode, round_up(size, PAGE_SIZE));
	if (!err)
		err = fxfs_dax_clean(inode, size, 1);
	/*
	 * What the last page holds past the end reads as zeros if the file grows again. A slice
	 * that can't take the zeros where they are is dropped, to be mapped again.
	 */
	while (!err) {
		err = dax_truncate_page(inode, size, &zeroed, &fxfs_dax_iomap_ops);
		if (err != -EXDEV)
			break;
		err = fxfs_dax_drop(inode, size >> FXFS_DAX_SHIFT, size >> FXFS_DAX_SHIFT, -1);
	}
	if (err)
		filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

void fxfs_dax_resize_end(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	unsigned long first = DIV_ROUND_UP(size, PAGE_SIZE), page, chunk;
	struct fxfs_dax_range *range;
	int err;

	down_write(&file->sem);
	xa_for_each_start(&file->ranges, chunk, range, size >> FXFS_DAX_SHIFT) {
		loff_t start = fxfs_dax_chunk_start(range);

		if (start >= round_up(size, PAGE_SIZE)) {
			fxfs_dax_unmap_range(dax, range);
			continue;
		}
		page = first - (start >> PAGE_SHIFT);
		if (page >= range->pages ||
		    find_next_bit(range->mapped, range->pages, page) == range->pages)
			continue;
		/* A slice's pages past the end stay its region's: the slice no longer maps them. */
		err = range->region ? 0 :
		      fxfs_device_unmap(dax->dev, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(range->pages - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		bitmap_clear(range->mapped, page, range->pages - page);
		bitmap_clear(range->writable, page, range->pages - page);
		bitmap_clear(range->relocated, page, range->pages - page);
		if (range->moved)
			memset(&range->moved[page], 0, (range->pages - page) * sizeof(*range->moved));
	}
	up_write(&file->sem);
	filemap_invalidate_unlock(inode->i_mapping);
}
