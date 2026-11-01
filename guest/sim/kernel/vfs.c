// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The VFS and page cache's calls dax.c and fs/dax.c make, rewritten for a DAX file alone, whose
 * mapping holds value entries, as mm/filemap.c, mm/truncate.c and mm/page-writeback.c treat them;
 * and the iov_iters over tasks' simulated memory.
 */

#include <sim/sim.h>
#include <sim/host.h>
#include <linux/dax.h>

/* As FXFS_DAX_MAX_DROPS. */
#define SIM_MAX_FAULT_DROPS 256

ssize_t generic_write_checks(struct kiocb *iocb, struct iov_iter *from)
{
	return iov_iter_count(from);
}

ssize_t generic_write_sync(struct kiocb *iocb, ssize_t count)
{
	return count;
}

void __mark_inode_dirty(struct inode *inode, int flags)
{
	inode->i_state |= flags;
}

struct inode *igrab(struct inode *inode)
{
	atomic_inc(&inode->i_count);
	return inode;
}

void iput(struct inode *inode)
{
	WARN_ON(atomic_dec_and_test(&inode->i_count));
}

bool noop_dirty_folio(struct address_space *mapping, struct folio *folio)
{
	return false;
}

ssize_t noop_direct_IO(struct kiocb *iocb, struct iov_iter *iter)
{
	return -EINVAL;
}

/* truncate_inode_pages_range's for value entries: truncate_folio_batch_exceptionals. */
void truncate_setsize(struct inode *inode, loff_t size)
{
	struct address_space *mapping = inode->i_mapping;
	pgoff_t first = DIV_ROUND_UP(size, PAGE_SIZE);
	unsigned long index;
	void *entry;

	i_size_write(inode, size);
	unmap_mapping_pages(mapping, first, ULONG_MAX, true);
	xa_for_each_start(&mapping->i_pages, index, entry, first) {
		if (xa_is_value(entry))
			dax_delete_mapping_entry(mapping, index);
	}
	unmap_mapping_pages(mapping, first, ULONG_MAX, true);
}

int invalidate_inode_pages2_range(struct address_space *mapping, pgoff_t start, pgoff_t end)
{
	unsigned long index;
	void *entry;
	int ret = 0;

	/*
	 * A fault going round for ever: only a seal would end it, if one came (TASK-140). dax.c has a
	 * write give up after FXFS_DAX_MAX_DROPS of these, and no fault.
	 */
	if (current->fault_drops >= 0 && ++current->fault_drops > SIM_MAX_FAULT_DROPS)
		sim_panic("livelock: a fault had its range dropped %d times", current->fault_drops);
	if (mapping_empty(mapping))
		return 0;
	xa_for_each_range(&mapping->i_pages, index, entry, start, end) {
		if (xa_is_value(entry) && !dax_invalidate_mapping_entry_sync(mapping, index))
			ret = -EBUSY;
	}
	unmap_mapping_pages(mapping, start, end - start + 1, false);
	return ret;
}

void tag_pages_for_writeback(struct address_space *mapping, pgoff_t start, pgoff_t end)
{
	XA_STATE(xas, &mapping->i_pages, start);
	void *page;

	xas_lock_irq(&xas);
	xas_for_each_marked(&xas, page, end, PAGECACHE_TAG_DIRTY)
		xas_set_mark(&xas, PAGECACHE_TAG_TOWRITE);
	xas_unlock_irq(&xas);
}

int filemap_fdatawrite_range(struct address_space *mapping, loff_t start, loff_t end)
{
	struct writeback_control wbc = {
		.sync_mode = WB_SYNC_ALL, .nr_to_write = LONG_MAX,
		.range_start = start, .range_end = end,
	};

	return mapping->a_ops->writepages(mapping, &wbc);
}

int filemap_write_and_wait_range(struct address_space *mapping, loff_t start, loff_t end)
{
	return filemap_fdatawrite_range(mapping, start, end);
}

size_t iov_iter_zero(size_t bytes, struct iov_iter *i)
{
	static const char zeros[PAGE_SIZE];
	size_t done = 0;

	bytes = min(bytes, i->count);
	if (!i->user) {
		memset(i->kbuf, 0, bytes);
		i->kbuf += bytes;
		i->count -= bytes;
		return bytes;
	}
	while (done < bytes) {
		size_t part = min_t(size_t, bytes - done, sizeof(zeros));
		size_t copied = sim_copy_user(i->uaddr, (void *)zeros, part, true);

		i->uaddr += copied;
		i->count -= copied;
		done += copied;
		if (copied < part)
			break;
	}
	return done;
}

/* Faults in the user's pages, one byte of each: what didn't, as the kernel's. */
static size_t fault_in(const struct iov_iter *i, size_t bytes, bool write)
{
	unsigned long addr = i->uaddr, end = i->uaddr + bytes;
	char byte = 0;

	if (!i->user)
		return 0;
	while (addr < end) {
		if (write) {
			/* A byte read and stored back, as fault_in_writeable's touch. */
			if (sim_copy_user(addr, &byte, 1, false) != 1 ||
			    sim_copy_user(addr, &byte, 1, true) != 1)
				return end - addr;
		} else if (sim_copy_user(addr, &byte, 1, false) != 1) {
			return end - addr;
		}
		addr = ALIGN(addr + 1, PAGE_SIZE);
	}
	return 0;
}

size_t fault_in_iov_iter_readable(const struct iov_iter *i, size_t bytes)
{
	return fault_in(i, bytes, false);
}

size_t fault_in_iov_iter_writeable(const struct iov_iter *i, size_t bytes)
{
	return fault_in(i, bytes, true);
}
