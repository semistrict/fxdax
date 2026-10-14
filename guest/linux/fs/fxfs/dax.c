// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Files' data mapped from the extent-store device's DAX window (backlog doc-2), as virtio-fs's DAX
 * maps a host's files (fs/fuse/dax.c), instead of the page cache: reads, writes and mappings of
 * files reach the extents the host shares, through the window.
 *
 * The window is split into ranges, each mapping a chunk of FXFS_DAX_SIZE bytes of one file: the
 * extents holding the chunk's data, page by page, read-only, or writably for open extents (doc-4),
 * and holes left out. A range is as large as its chunk of the file, in pages: a small file's takes
 * what it needs of the window. A write first makes room for its data in open extents
 * (fxfs_dax_map_write), which the range then maps writably, and stores through the mapping;
 * mappings of files map the window's pages. A file's page stays at one window address, whatever
 * extent holds it: a block that moves is mapped again there.
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
#include <linux/iomap.h>
#include <linux/mm.h>
#include <linux/pagemap.h>
#include <linux/pfn_t.h>
#include <linux/slab.h>
#include <linux/uio.h>
#include <linux/wait.h>
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
	/* Its size, in pages: its chunk's, up to the file's end when it was mapped. */
	unsigned long pages;
	/* The iomaps using it, which keep it. */
	atomic_t users;
	/* Its pages that map an extent, and those that map one writably. */
	unsigned long *mapped;
	unsigned long *writable;
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
};

static const struct iomap_ops fxfs_dax_iomap_ops;
static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;
static int fxfs_dax_break_layouts(struct inode *inode, loff_t start);

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
 * Maps the pieces `extents` of the chunk `range` maps, at their places in it: open extents
 * writably. In the gate, with the inode's DAX semaphore held for writing.
 */
static int fxfs_dax_map_extents(struct fxfs_dax *dax, struct fxfs_dax_range *range,
				const struct fxfs_dax_extent *extents, ssize_t count)
{
	loff_t start = fxfs_dax_chunk_start(range);
	bool writable = false;
	ssize_t i;
	int err;

	for (i = 0; i < count; i++) {
		const struct fxfs_dax_extent *e = &extents[i];
		unsigned long page = (e->offset - start) >> PAGE_SHIFT;
		unsigned long pages = e->len >> PAGE_SHIFT;

		err = fxfs_device_map(dax->dev, e->branch, e->sequence, e->extent_offset, e->len,
				      range->window_offset + (e->offset - start), e->open);
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

/* Maps the extents holding the data of the chunk `range` is for, which is unmapped. */
static int fxfs_dax_map_chunk(struct fxfs_dax *dax, struct inode *inode,
			      struct fxfs_dax_range *range)
{
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = min_t(loff_t, start + ((loff_t)range->pages << PAGE_SHIFT),
			   round_up(i_size_read(inode), PAGE_SIZE));
	struct fxfs_dax_extent *extents;
	ssize_t count;
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
		if (fxfs_device_store_begin(dax->dev) == seals) {
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
 * A new range of `pages` pages of the window, unmapped and on no list, or NULL if the window has no
 * room for it. A whole chunk's range is at a chunk's alignment in the window.
 */
static struct fxfs_dax_range *fxfs_dax_new_range(struct fxfs_dax *dax, unsigned long pages)
{
	size_t size = pages << PAGE_SHIFT;
	struct fxfs_dax_range *range;
	unsigned long addr;

	range = kzalloc(sizeof(*range), GFP_NOFS);
	if (!range)
		return ERR_PTR(-ENOMEM);
	range->mapped = bitmap_zalloc(pages, GFP_NOFS);
	range->writable = bitmap_zalloc(pages, GFP_NOFS);
	if (!range->mapped || !range->writable) {
		bitmap_free(range->mapped);
		bitmap_free(range->writable);
		kfree(range);
		return ERR_PTR(-ENOMEM);
	}
	if (pages == FXFS_DAX_PAGES)
		addr = gen_pool_alloc_algo(dax->pool, size, gen_pool_first_fit_order_align, NULL);
	else
		addr = gen_pool_alloc(dax->pool, size);
	if (!addr) {
		bitmap_free(range->mapped);
		bitmap_free(range->writable);
		kfree(range);
		return NULL;
	}
	range->window_offset = addr - FXFS_DAX_POOL_BASE;
	range->pages = pages;
	INIT_LIST_HEAD(&range->list);
	return range;
}

/* Frees `range`, which nothing maps: its space in the window is free again. */
static void fxfs_dax_free_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	spin_lock(&dax->lock);
	list_del(&range->list);
	spin_unlock(&dax->lock);
	gen_pool_free(dax->pool, range->window_offset + FXFS_DAX_POOL_BASE,
		      range->pages << PAGE_SHIFT);
	bitmap_free(range->mapped);
	bitmap_free(range->writable);
	kfree(range);
	fxfs_dax_released(dax);
}

/*
 * Unmaps `range` and frees it, with its file's DAX semaphore held for writing, once nothing maps
 * its pages: its file's inode's mappings of them are gone (fxfs_dax_release), if it has one.
 */
static void fxfs_dax_unmap_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	int err;

	xa_erase(&range->file->ranges, range->chunk);
	if (!bitmap_empty(range->mapped, range->pages)) {
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
 * A new range of `pages` pages, reclaimed if the window has no room. Waits for one, unless a fault
 * asks, which can't: -EAGAIN then.
 */
static struct fxfs_dax_range *fxfs_dax_alloc_range(struct fxfs_dax *dax, unsigned long pages,
						   bool fault)
{
	struct fxfs_dax_range *range;
	long seen;

	for (;;) {
		seen = atomic_long_read(&dax->released);
		range = fxfs_dax_new_range(dax, pages);
		if (range)
			return range;
		if (fxfs_dax_reclaim(dax))
			continue;
		if (fault)
			return ERR_PTR(-EAGAIN);
		/* Ranges in use, or whose inodes are locked, free up soon. */
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return ERR_PTR(-EINTR);
	}
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
 * The range mapping `inode`'s chunk `chunk`, mapped first if it isn't, for a file at least `size`
 * bytes long, kept for an iomap.
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

	pages = fxfs_dax_range_pages(chunk, max(i_size_read(inode), size));
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
		err = fxfs_dax_map_chunk(dax, inode, new);
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
 * Drops the ranges of `inode`'s file too small for it to be `size` bytes long: those of the chunks
 * its end moves past or into, which are mapped again as large as they need. With the inode's lock
 * and invalidate_lock held for writing.
 */
static int fxfs_dax_fit(struct inode *inode, loff_t size)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	loff_t old = i_size_read(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;
	int err;

	if (size <= old)
		return 0;
	for (;;) {
		long seen = atomic_long_read(&dax->released);
		bool busy = false;

		down_write(&file->sem);
		xa_for_each_range(&file->ranges, chunk, range, old >> FXFS_DAX_SHIFT,
				  (size - 1) >> FXFS_DAX_SHIFT) {
			if (range->pages >= fxfs_dax_range_pages(chunk, size))
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
		err = fxfs_dax_break_layouts(inode, (loff_t)(old >> FXFS_DAX_SHIFT) << FXFS_DAX_SHIFT);
		if (err)
			return err;
		if (fxfs_dax_wait_released(dax, seen, 10) < 0)
			return -EINTR;
	}
}

/* Fills in `iomap` for a hole of `length` bytes. */
static void fxfs_dax_hole(struct iomap *iomap, loff_t length)
{
	iomap->type = IOMAP_HOLE;
	iomap->addr = IOMAP_NULL_ADDR;
	iomap->length = length;
}

/* Maps a read of up to `length` bytes at `pos`: a run of mapped pages, or of holes. */
static int fxfs_dax_iomap_read(struct inode *inode, loff_t pos, loff_t length, unsigned int flags,
			       struct iomap *iomap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long page, end_page;
	loff_t start;
	bool mapped;

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
	up_read(&file->sem);
	length = min_t(loff_t, length, start + ((loff_t)end_page << PAGE_SHIFT) - pos);
	if (!mapped) {
		fxfs_dax_put_range(dax, range);
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	iomap->type = IOMAP_MAPPED;
	iomap->addr = range->window_offset + (pos - start);
	iomap->length = length;
	iomap->private = range;
	return 0;
}

/*
 * Maps a write of up to `length` bytes at `pos` writably, making room for it in open extents
 * unless its pages are writable already, and returns in the gate, which fxfs_dax_iomap_end leaves.
 * A zeroing write leaves holes be. A write past the file's end grows it, unless it is a fault's.
 */
static int fxfs_dax_iomap_write(struct inode *inode, loff_t pos, loff_t length,
				unsigned int flags, struct iomap *iomap)
{
	struct fxfs_dax_file *file = fxfs_dax_file(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_extent *extents = NULL;
	struct fxfs_dax_range *range;
	unsigned long first, pages;
	loff_t start, end, size;
	bool fresh = false;
	ssize_t count;
	u64 seals;
	int err = 0;

	/* Faults and zeroing don't grow the file. */
	size = flags & (IOMAP_FAULT | IOMAP_ZERO) ? 0 :
	       min_t(loff_t, pos + length, round_up(pos + 1, FXFS_DAX_SIZE));
	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, size, flags & IOMAP_FAULT);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	end = min_t(loff_t, pos + length, start + ((loff_t)range->pages << PAGE_SHIFT));
	/* A range covers the file's chunk as its size grows (fxfs_dax_fit). */
	if (WARN_ON_ONCE(end <= pos)) {
		fxfs_dax_put_range(dax, range);
		return -EIO;
	}
	if (size)
		size = end;
	first = (pos - start) >> PAGE_SHIFT;
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;

	down_write(&file->sem);
	for (;;) {
		fxfs_device_store_begin(dax->dev);
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
		/* As in fxfs_dax_map_chunk: unless a seal was in progress, or began since. */
		seals = fxfs_device_seals(dax->dev);
		/* A fault's pages hold what they held: a store may change any part of them. */
		count = fxfs_nofs(fxfs_dax_map_write(dax->fs, inode->i_ino, pos, end - pos, size,
						     flags & IOMAP_FAULT, extents,
						     FXFS_DAX_PAGES));
		if (count < 0) {
			err = count;
			goto out;
		}
		if (size > i_size_read(inode))
			i_size_write(inode, size);
		if (fxfs_device_store_begin(dax->dev) == seals) {
			err = fxfs_dax_map_extents(dax, range, extents, count);
			if (err) {
				fxfs_device_store_end(dax->dev);
				goto out;
			}
			break;
		}
		fxfs_device_store_end(dax->dev);
	}
	/* In the gate. */
	up_write(&file->sem);
	kvfree(extents);
	iomap->type = IOMAP_MAPPED;
	/* Holes a mapping of the file maps zeros for are gone. */
	iomap->flags |= IOMAP_F_PRIVATE | (fresh ? IOMAP_F_NEW : 0);
	iomap->addr = range->window_offset + (pos - start);
	iomap->length = end - pos;
	iomap->private = range;
	return 0;
out:
	up_write(&file->sem);
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
		return fxfs_dax_iomap_write(inode, pos, length, flags, iomap);
	return fxfs_dax_iomap_read(inode, pos, length, flags, iomap);
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
		if (ret > 0)
			done += ret;
		else if (ret != -EFAULT)
			break;
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
		if (!(ret & VM_FAULT_ERROR) || error != -EAGAIN)
			break;
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

/* Writes back the file's times, and makes the filesystem durable, sealing what was stored. */
static int fxfs_dax_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file_inode(file);
	int err = file_write_and_wait_range(file, start, end);

	if (!err)
		err = sync_inode_metadata(inode, 1);
	if (!err)
		err = fxfs_nofs(fxfs_sync(fxfs_vfs_fs(inode->i_sb)));
	return err;
}

static const struct file_operations fxfs_dax_file_operations = {
	.open = generic_file_open,
	.llseek = generic_file_llseek,
	.read_iter = fxfs_dax_read_iter,
	.write_iter = fxfs_dax_write_iter,
	.mmap = fxfs_dax_mmap,
	.splice_read = copy_splice_read,
	.splice_write = iter_file_splice_write,
	.fsync = fxfs_dax_fsync,
};

/*
 * Before a seal: makes user mappings of writable pages read-only, and forgets which pages are
 * writable, with no store in progress.
 */
static void fxfs_dax_seal(void *arg)
{
	struct fxfs_dax *dax = arg;
	struct fxfs_dax_file *file, *next;
	struct fxfs_dax_range *range;
	unsigned long chunk;

	mutex_lock(&dax->writable_lock);
	list_for_each_entry_safe(file, next, &dax->writable, writable) {
		/* A file with no inode has nothing mapping its pages but the window. */
		mutex_lock(&dax->files_lock);
		if (file->inode)
			wp_shared_mapping_range(file->inode->i_mapping, 0, ULONG_MAX);
		mutex_unlock(&dax->files_lock);
		xa_for_each(&file->ranges, chunk, range)
			bitmap_zero(range->writable, range->pages);
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
	fxfs_device_set_sealer(dev, fxfs_dax_seal, dax);
	return dax;
}

/*
 * Frees `dax` once the filesystem is unmounted, its inodes evicted: the window's ranges are
 * unmapped as the device goes.
 */
void fxfs_dax_destroy(struct fxfs_dax *dax)
{
	struct fxfs_dax_range *range, *next;
	struct fxfs_dax_file *file;
	unsigned long ino;

	list_for_each_entry_safe(range, next, &dax->used, list) {
		xa_erase(&range->file->ranges, range->chunk);
		fxfs_dax_free_range(dax, range);
	}
	xa_for_each(&dax->files, ino, file) {
		xa_erase(&dax->files, ino);
		fxfs_dax_put_file(dax, file);
	}
	xa_destroy(&dax->files);
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
	/* What the last page holds past the end reads as zeros if the file grows again. */
	if (!err)
		err = dax_truncate_page(inode, size, &zeroed, &fxfs_dax_iomap_ops);
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
		err = fxfs_device_unmap(dax->dev, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(range->pages - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		bitmap_clear(range->mapped, page, range->pages - page);
		bitmap_clear(range->writable, page, range->pages - page);
	}
	up_write(&file->sem);
	filemap_invalidate_unlock(inode->i_mapping);
}
