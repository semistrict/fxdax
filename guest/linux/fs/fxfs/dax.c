// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Files' data mapped from the extent-store device's DAX window (backlog doc-2), as virtio-fs's DAX
 * maps a host's files (fs/fuse/dax.c), instead of the page cache: reads, writes and mappings of
 * files reach the extents the host shares, through the window.
 *
 * The window is split into ranges of FXFS_DAX_SIZE bytes, each mapping a chunk of one file: the
 * extents holding the chunk's data, page by page, read-only, or writably for open extents (doc-4),
 * and holes left out. A write first makes room for its data in open extents (fxfs_dax_map_write),
 * which the range then maps writably, and stores through the mapping; mappings of files map the
 * window's pages. Ranges in use stay; others are reclaimed when the window runs out.
 *
 * Requests that seal open extents make their writable mappings read-only. Stores are made in the
 * device's gate (device.h), which such requests wait for; before them, the sealer makes user
 * mappings of writable pages read-only, so that a store faults, and forgets which pages are
 * writable, so that a write makes room again. Kernel copies through the window are made with page
 * faults disabled, as one waiting for the gate to open may hold what a seal needs.
 *
 * Locks are taken in this order: an inode's lock (read and write) or its invalidate_lock (faults,
 * truncation and reclaim), then its DAX semaphore, then the gate. Another inode's are only tried,
 * when reclaiming its ranges. Rust is called holding no gate.
 */

#include <linux/dax.h>
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

/* A range of the window. */
struct fxfs_dax_range {
	/* On the filesystem's free or used list. */
	struct list_head list;
	/* The inode whose chunk it maps, and which, or NULL. */
	struct inode *inode;
	pgoff_t chunk;
	u64 window_offset;
	/* The iomaps using it, which keep it. */
	atomic_t users;
	/* Its pages that map an extent, and those that map one writably. */
	DECLARE_BITMAP(mapped, FXFS_DAX_PAGES);
	DECLARE_BITMAP(writable, FXFS_DAX_PAGES);
};

struct fxfs_dax {
	struct super_block *sb;
	struct fxfs *fs;
	struct fxfs_device *dev;
	struct dax_device *dax_dev;
	struct fxfs_dax_range *ranges;
	unsigned long nr_ranges;
	/* Guards the free and used lists. */
	spinlock_t lock;
	struct list_head free;
	/* The ranges mapping chunks, least recently used first. */
	struct list_head used;
	/* Woken as ranges are freed, or their last users done. */
	wait_queue_head_t wait;
	/* Guards `writable`: the inodes with writable pages in the window. */
	struct mutex writable_lock;
	struct list_head writable;
};

static const struct iomap_ops fxfs_dax_iomap_ops;
static const struct address_space_operations fxfs_dax_aops;
static const struct file_operations fxfs_dax_file_operations;

static struct fxfs_dax *fxfs_dax_of(struct inode *inode)
{
	return fxfs_vfs_info(inode->i_sb)->dax;
}

static struct fxfs_dax_inode *fxfs_dax_inode(struct inode *inode)
{
	return &fxfs_vfs_inode(inode)->dax;
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

/* Marks `inode` as having writable pages in the window, which the sealer forgets. In the gate. */
static void fxfs_dax_add_writable(struct fxfs_dax *dax, struct inode *inode)
{
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);

	mutex_lock(&dax->writable_lock);
	if (list_empty(&di->writable))
		list_add_tail(&di->writable, &dax->writable);
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
		fxfs_dax_add_writable(dax, range->inode);
	return 0;
}

/* Maps the extents holding the data of the chunk `range` is for, which is unmapped. */
static int fxfs_dax_map_chunk(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	struct inode *inode = range->inode;
	loff_t start = fxfs_dax_chunk_start(range);
	loff_t end = min_t(loff_t, start + FXFS_DAX_SIZE, round_up(i_size_read(inode), PAGE_SIZE));
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

/* Puts `range`, free, on the free list. */
static void fxfs_dax_free_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	range->inode = NULL;
	bitmap_zero(range->mapped, FXFS_DAX_PAGES);
	bitmap_zero(range->writable, FXFS_DAX_PAGES);
	spin_lock(&dax->lock);
	list_move(&range->list, &dax->free);
	spin_unlock(&dax->lock);
	wake_up_all(&dax->wait);
}

/*
 * Unmaps `range` and frees it, with its inode's DAX semaphore held for writing, once nothing
 * maps its pages. Its inode's mappings of them are gone (fxfs_dax_release).
 */
static void fxfs_dax_unmap_range(struct fxfs_dax *dax, struct fxfs_dax_range *range)
{
	struct fxfs_dax_inode *di = fxfs_dax_inode(range->inode);
	int err;

	xa_erase(&di->ranges, range->chunk);
	if (!bitmap_empty(range->mapped, FXFS_DAX_PAGES)) {
		err = fxfs_device_unmap(dax->dev, range->window_offset, FXFS_DAX_SIZE);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
	}
	fxfs_dax_free_range(dax, range);
}

/*
 * Removes the inode's mappings of the chunk `range` maps, and the DAX entries for them, unless
 * a page is in use (pinned): with the inode's invalidate_lock held for writing.
 */
static bool fxfs_dax_release(struct inode *inode, struct fxfs_dax_range *range)
{
	struct address_space *mapping = inode->i_mapping;
	loff_t start = fxfs_dax_chunk_start(range), end = start + FXFS_DAX_SIZE - 1;

	if (dax_layout_busy_page_range(mapping, start, end))
		return false;
	/* Dirty entries go only once written back, which only cleans them. */
	if (filemap_fdatawrite_range(mapping, start, end))
		return false;
	return !invalidate_inode_pages2_range(mapping, start >> PAGE_SHIFT, end >> PAGE_SHIFT);
}

/*
 * Reclaims the least recently used range no iomap uses, if its inode's locks are free, and
 * returns whether it did.
 */
static bool fxfs_dax_reclaim(struct fxfs_dax *dax)
{
	struct fxfs_dax_range *range, *found = NULL;
	struct fxfs_dax_inode *di;
	struct inode *inode = NULL;
	pgoff_t chunk = 0;
	bool reclaimed = false;

	spin_lock(&dax->lock);
	list_for_each_entry(range, &dax->used, list) {
		if (atomic_read(&range->users))
			continue;
		inode = igrab(range->inode);
		if (!inode)
			continue;
		/* Tried once: the next try starts with another. */
		list_move_tail(&range->list, &dax->used);
		chunk = range->chunk;
		found = range;
		break;
	}
	spin_unlock(&dax->lock);
	if (!found)
		return false;

	di = fxfs_dax_inode(inode);
	if (down_write_trylock(&inode->i_mapping->invalidate_lock)) {
		if (down_write_trylock(&di->sem)) {
			range = xa_load(&di->ranges, chunk);
			if (range && !atomic_read(&range->users) && fxfs_dax_release(inode, range)) {
				fxfs_dax_unmap_range(dax, range);
				reclaimed = true;
			}
			up_write(&di->sem);
		}
		filemap_invalidate_unlock(inode->i_mapping);
	}
	iput(inode);
	return reclaimed;
}

/*
 * A free range, reclaimed if none is. Waits for one, unless a fault asks, which can't: -EAGAIN
 * then.
 */
static struct fxfs_dax_range *fxfs_dax_alloc_range(struct fxfs_dax *dax, bool fault)
{
	struct fxfs_dax_range *range;

	for (;;) {
		spin_lock(&dax->lock);
		range = list_first_entry_or_null(&dax->free, struct fxfs_dax_range, list);
		if (range)
			list_del_init(&range->list);
		spin_unlock(&dax->lock);
		if (range)
			return range;
		if (fxfs_dax_reclaim(dax))
			continue;
		if (fault)
			return ERR_PTR(-EAGAIN);
		/* Ranges in use, or whose inodes are locked, free up soon. */
		if (wait_event_killable_timeout(dax->wait, !list_empty_careful(&dax->free),
						msecs_to_jiffies(10)) < 0)
			return ERR_PTR(-EINTR);
	}
}

/* Waits a moment for a range to be freed, for a fault that found none. */
static void fxfs_dax_wait_free(struct fxfs_dax *dax)
{
	wait_event_timeout(dax->wait, !list_empty_careful(&dax->free), msecs_to_jiffies(10));
}

/* The range mapping `inode`'s chunk `chunk`, mapped first if it isn't, kept for an iomap. */
static struct fxfs_dax_range *fxfs_dax_get_range(struct inode *inode, pgoff_t chunk, bool fault)
{
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range, *new;
	int err;

	down_read(&di->sem);
	range = xa_load(&di->ranges, chunk);
	if (range)
		atomic_inc(&range->users);
	up_read(&di->sem);
	if (range)
		goto used;

	new = fxfs_dax_alloc_range(dax, fault);
	if (IS_ERR(new))
		return new;
	down_write(&di->sem);
	/* Another iomap may have mapped it meanwhile. */
	range = xa_load(&di->ranges, chunk);
	if (range) {
		atomic_inc(&range->users);
		up_write(&di->sem);
		fxfs_dax_free_range(dax, new);
		goto used;
	}
	new->inode = inode;
	new->chunk = chunk;
	atomic_set(&new->users, 1);
	err = xa_err(xa_store(&di->ranges, chunk, new, GFP_NOFS));
	if (!err) {
		err = fxfs_dax_map_chunk(dax, new);
		if (err)
			fxfs_dax_unmap_range(dax, new);
	} else {
		fxfs_dax_free_range(dax, new);
	}
	up_write(&di->sem);
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
		wake_up_all(&dax->wait);
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
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long page, end_page;
	loff_t start;
	bool mapped;

	if (pos >= i_size_read(inode)) {
		fxfs_dax_hole(iomap, length);
		return 0;
	}
	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, flags & IOMAP_FAULT);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	page = (pos - start) >> PAGE_SHIFT;
	down_read(&di->sem);
	mapped = test_bit(page, range->mapped);
	end_page = mapped ? find_next_zero_bit(range->mapped, FXFS_DAX_PAGES, page) :
			    find_next_bit(range->mapped, FXFS_DAX_PAGES, page);
	up_read(&di->sem);
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
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_extent *extents = NULL;
	struct fxfs_dax_range *range;
	unsigned long first, pages;
	loff_t start, end, size;
	bool fresh = false;
	ssize_t count;
	u64 seals;
	int err = 0;

	range = fxfs_dax_get_range(inode, pos >> FXFS_DAX_SHIFT, flags & IOMAP_FAULT);
	if (IS_ERR(range))
		return PTR_ERR(range);
	start = fxfs_dax_chunk_start(range);
	end = min_t(loff_t, pos + length, start + FXFS_DAX_SIZE);
	first = (pos - start) >> PAGE_SHIFT;
	pages = DIV_ROUND_UP(end - start, PAGE_SIZE) - first;
	/* Faults and zeroing don't grow the file. */
	size = flags & (IOMAP_FAULT | IOMAP_ZERO) ? 0 : end;

	down_write(&di->sem);
	for (;;) {
		fxfs_device_store_begin(dax->dev);
		/* Writable pages take the write as they are, unless it grows the file. */
		if (find_next_zero_bit(range->writable, first + pages, first) == first + pages &&
		    size <= i_size_read(inode))
			break;
		if (flags & IOMAP_ZERO && find_next_bit(range->mapped, first + pages, first) ==
						  first + pages) {
			fxfs_device_store_end(dax->dev);
			up_write(&di->sem);
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
	up_write(&di->sem);
	kvfree(extents);
	iomap->type = IOMAP_MAPPED;
	/* Holes a mapping of the file maps zeros for are gone. */
	iomap->flags |= IOMAP_F_PRIVATE | (fresh ? IOMAP_F_NEW : 0);
	iomap->addr = range->window_offset + (pos - start);
	iomap->length = end - pos;
	iomap->private = range;
	return 0;
out:
	up_write(&di->sem);
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
		error = 0;
		filemap_invalidate_lock_shared(inode->i_mapping);
		ret = dax_iomap_fault(vmf, 0, &pfn, &error, &fxfs_dax_iomap_ops);
		filemap_invalidate_unlock_shared(inode->i_mapping);
		if (!(ret & VM_FAULT_ERROR) || error != -EAGAIN)
			break;
		/* No range was free, nor could the fault reclaim one, holding the lock it holds. */
		fxfs_dax_wait_free(fxfs_dax_of(inode));
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
	struct fxfs_dax_inode *di, *next;
	struct fxfs_dax_range *range;
	unsigned long chunk;

	mutex_lock(&dax->writable_lock);
	list_for_each_entry_safe(di, next, &dax->writable, writable) {
		struct inode *inode = &container_of(di, struct fxfs_vfs_inode, dax)->vfs;

		wp_shared_mapping_range(inode->i_mapping, 0, ULONG_MAX);
		xa_for_each(&di->ranges, chunk, range)
			bitmap_zero(range->writable, FXFS_DAX_PAGES);
		list_del_init(&di->writable);
	}
	mutex_unlock(&dax->writable_lock);
}

struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs)
{
	struct fxfs_device *dev = fxfs_dax_device(fs);
	struct dax_device *dax_dev;
	struct fxfs_dax *dax;
	u64 window_len;
	unsigned long i;

	dax_dev = fxfs_device_dax(dev, &window_len);
	if (!dax_dev || window_len < FXFS_DAX_SIZE)
		return NULL;
	dax = kzalloc(sizeof(*dax), GFP_KERNEL);
	if (!dax)
		return ERR_PTR(-ENOMEM);
	dax->nr_ranges = window_len >> FXFS_DAX_SHIFT;
	dax->ranges = kvcalloc(dax->nr_ranges, sizeof(*dax->ranges), GFP_KERNEL);
	if (!dax->ranges) {
		kfree(dax);
		return ERR_PTR(-ENOMEM);
	}
	dax->sb = sb;
	dax->fs = fs;
	dax->dev = dev;
	dax->dax_dev = dax_dev;
	spin_lock_init(&dax->lock);
	INIT_LIST_HEAD(&dax->free);
	INIT_LIST_HEAD(&dax->used);
	init_waitqueue_head(&dax->wait);
	mutex_init(&dax->writable_lock);
	INIT_LIST_HEAD(&dax->writable);
	for (i = 0; i < dax->nr_ranges; i++) {
		dax->ranges[i].window_offset = (u64)i << FXFS_DAX_SHIFT;
		list_add_tail(&dax->ranges[i].list, &dax->free);
	}
	fxfs_device_set_sealer(dev, fxfs_dax_seal, dax);
	return dax;
}

void fxfs_dax_destroy(struct fxfs_dax *dax)
{
	kvfree(dax->ranges);
	kfree(dax);
}

void fxfs_dax_inode_init(struct fxfs_dax_inode *di)
{
	init_rwsem(&di->sem);
	xa_init(&di->ranges);
	INIT_LIST_HEAD(&di->writable);
}

void fxfs_dax_set_inode(struct inode *inode)
{
	inode->i_flags |= S_DAX;
	inode->i_fop = &fxfs_dax_file_operations;
	inode->i_mapping->a_ops = &fxfs_dax_aops;
}

void fxfs_dax_evict(struct inode *inode)
{
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	struct fxfs_dax_range *range;
	unsigned long chunk;

	if (!dax || !IS_DAX(inode))
		return;
	mutex_lock(&dax->writable_lock);
	list_del_init(&di->writable);
	mutex_unlock(&dax->writable_lock);
	down_write(&di->sem);
	xa_for_each(&di->ranges, chunk, range)
		fxfs_dax_unmap_range(dax, range);
	up_write(&di->sem);
	xa_destroy(&di->ranges);
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
	if (size >= i_size_read(inode))
		return 0;
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
	struct fxfs_dax_inode *di = fxfs_dax_inode(inode);
	struct fxfs_dax *dax = fxfs_dax_of(inode);
	unsigned long first = DIV_ROUND_UP(size, PAGE_SIZE), page, chunk;
	struct fxfs_dax_range *range;
	int err;

	down_write(&di->sem);
	xa_for_each_start(&di->ranges, chunk, range, size >> FXFS_DAX_SHIFT) {
		loff_t start = fxfs_dax_chunk_start(range);

		if (start >= round_up(size, PAGE_SIZE)) {
			fxfs_dax_unmap_range(dax, range);
			continue;
		}
		page = first - (start >> PAGE_SHIFT);
		if (find_next_bit(range->mapped, FXFS_DAX_PAGES, page) == FXFS_DAX_PAGES)
			continue;
		err = fxfs_device_unmap(dax->dev, range->window_offset + ((u64)page << PAGE_SHIFT),
					(u64)(FXFS_DAX_PAGES - page) << PAGE_SHIFT);
		if (err)
			pr_warn("fxfs: unmapping the DAX window at %llx failed: %d\n",
				range->window_offset, err);
		bitmap_clear(range->mapped, page, FXFS_DAX_PAGES - page);
		bitmap_clear(range->writable, page, FXFS_DAX_PAGES - page);
	}
	up_write(&di->sem);
	filemap_invalidate_unlock(inode->i_mapping);
}
