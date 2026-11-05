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

/* As the kernel's: none of an inode being evicted, which goes once the last reference does. */
struct inode *igrab(struct inode *inode)
{
	if (inode->i_state & I_FREEING)
		return NULL;
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

/*
 * fs/remap_range.c's, for files within a test's: no checks, and both ranges written back, as the
 * VFS writes them back before a filesystem shares their blocks.
 */
int generic_remap_file_range_prep(struct file *file_in, loff_t pos_in, struct file *file_out,
				  loff_t pos_out, loff_t *len, unsigned int remap_flags)
{
	int err = filemap_write_and_wait_range(file_in->f_mapping, pos_in, pos_in + *len - 1);

	return err ?: filemap_write_and_wait_range(file_out->f_mapping, pos_out,
						   pos_out + *len - 1);
}

/* By address, as the kernel's. */
void lock_two_nondirectories(struct inode *inode1, struct inode *inode2)
{
	inode_lock(min(inode1, inode2));
	if (inode1 != inode2)
		inode_lock(max(inode1, inode2));
}

void unlock_two_nondirectories(struct inode *inode1, struct inode *inode2)
{
	if (inode1 != inode2)
		inode_unlock(max(inode1, inode2));
	inode_unlock(min(inode1, inode2));
}

void filemap_invalidate_lock_two(struct address_space *mapping1, struct address_space *mapping2)
{
	filemap_invalidate_lock(min(mapping1, mapping2));
	if (mapping1 != mapping2)
		filemap_invalidate_lock(max(mapping1, mapping2));
}

void filemap_invalidate_unlock_two(struct address_space *mapping1,
				   struct address_space *mapping2)
{
	if (mapping1 != mapping2)
		filemap_invalidate_unlock(max(mapping1, mapping2));
	filemap_invalidate_unlock(min(mapping1, mapping2));
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
	 * write and a fault give up after FXFS_DAX_MAX_DROPS of these: more is that limit gone.
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

/*
 * As filemap_fdatawrite_wbc: writepages only if the mapping has dirty entries, which a DAX file's
 * write faults tag, and its write(2)s don't.
 */
int filemap_fdatawrite_range(struct address_space *mapping, loff_t start, loff_t end)
{
	struct writeback_control wbc = {
		.sync_mode = WB_SYNC_ALL, .nr_to_write = LONG_MAX,
		.range_start = start, .range_end = end,
	};

	if (!mapping_tagged(mapping, PAGECACHE_TAG_DIRTY))
		return 0;
	return mapping->a_ops->writepages(mapping, &wbc);
}

/* With the error a writeback recorded since the last (mapping_set_error), which it clears. */
int filemap_write_and_wait_range(struct address_space *mapping, loff_t start, loff_t end)
{
	int err = filemap_fdatawrite_range(mapping, start, end);
	int recorded = xchg(&mapping->wb_err, 0);

	return err ?: recorded;
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

void iov_iter_kvec(struct iov_iter *i, unsigned int direction, const struct kvec *kvec,
		   unsigned long nr_segs, size_t count)
{
	if (nr_segs != 1)
		sim_panic("%lu segments of a kvec", nr_segs);
	*i = (struct iov_iter) { .data_source = direction, .count = count, .kbuf = kvec->iov_base };
}

void iov_iter_revert(struct iov_iter *i, size_t bytes)
{
	i->count += bytes;
	if (i->user)
		i->uaddr -= bytes;
	else
		i->kbuf -= bytes;
}

/* What it copied, which stops short where the user's memory does. */
size_t copy_from_iter(void *addr, size_t bytes, struct iov_iter *i)
{
	size_t done;

	bytes = min(bytes, i->count);
	if (i->user) {
		done = sim_copy_user(i->uaddr, addr, bytes, false);
		i->uaddr += done;
	} else {
		memcpy(addr, i->kbuf, bytes);
		i->kbuf += bytes;
		done = bytes;
	}
	i->count -= done;
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
