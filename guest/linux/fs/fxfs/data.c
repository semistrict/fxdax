// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * fxdax: changes of a regular file as a whole: its size (truncate, fallocate, a DAX write stopped
 * short), holes punched, ranges reflinked. Each is one sequence whatever holds the file's data,
 * the page cache or the DAX window, which does its own around it (vfs.h's fxfs_data_ops): so that
 * guest/sim, which builds this as it is, with dax.c's operations, tests what the kernel runs
 * (TASK-160.2). Its copies of them there had drifted: a resize that failed to grow kept the size,
 * and a clone that failed lost what it reached.
 */

#include <linux/fs.h>
/* After fs.h, whose types its uapi half takes. */
#include <linux/falloc.h>
#include <linux/pagemap.h>

#include "fxfs.h"
#include "vfs.h"

static const struct fxfs_data_ops *fxfs_data_ops(struct inode *inode)
{
	return fxfs_vfs_inode(inode)->data;
}

/*
 * Makes `inode` `size` bytes long, with the attributes `iattr` sets, if any, and writes them back.
 * Its inode lock held. fxdax: every change of a file's size goes here: a DAX write stopped short
 * made its own, telling Fxfs nothing of the window's last block, which Fxfs moved from under it to
 * zero what it held past the end (TASK-150.9).
 */
int fxfs_data_resize(struct inode *inode, loff_t size, struct mnt_idmap *idmap, struct iattr *iattr)
{
	const struct fxfs_data_ops *ops = fxfs_data_ops(inode);
	loff_t old_size = i_size_read(inode);
	int err = ops->resize_begin(inode, size);

	if (err)
		return err;
	/* The page cache past the size goes first, so writeback can't write it back. */
	truncate_setsize(inode, size);
	if (iattr)
		setattr_copy(idmap, inode, iattr);
	err = fxfs_vfs_write_attr(inode, FXFS_SET_SIZE | (ops->keeps_tail ? FXFS_SET_KEEP_TAIL : 0));
	/*
	 * fxdax: with no room, a file grown goes back to its size: nothing is past its old end yet,
	 * and the inode saying a size Fxfs hasn't would lose it at eviction. A cut borrows the room
	 * it needs (TASK-60), and its pages are gone either way.
	 */
	if (err && size > old_size)
		i_size_write(inode, old_size);
	if (ops->resize_end)
		ops->resize_end(inode, i_size_read(inode));
	return err;
}

/*
 * Makes `start` to `end` of `inode`, within its size, a hole: its partial pages zeroed, its whole
 * pages' blocks freed. With the inode's lock held.
 */
static int fxfs_data_punch(struct inode *inode, loff_t start, loff_t end)
{
	const struct fxfs_data_ops *ops = fxfs_data_ops(inode);
	loff_t first = round_up(start, PAGE_SIZE), last = round_down(end, PAGE_SIZE);
	int err;

	if (end <= start)
		return 0;
	filemap_invalidate_lock(inode->i_mapping);
	err = ops->punch_begin(inode, start, end);
	/* The page the hole is within, or its partial pages at either end. */
	if (!err && first > last) {
		err = ops->zero(inode, start, end - start);
	} else {
		if (!err && start < first)
			err = ops->zero(inode, start, first - start);
		if (!err && last < end)
			err = ops->zero(inode, last, end - last);
	}
	if (!err && first < last && ops->punch_drop)
		err = ops->punch_drop(inode, first, last);
	if (!err && first < last)
		err = fxfs_nofs(fxfs_punch(fxfs_vfs_fs(inode->i_sb), inode->i_ino, first,
					   last - first));
	filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

/*
 * fallocate(2): makes the file at least `offset` + `len` bytes long, unless FALLOC_FL_KEEP_SIZE;
 * with FALLOC_FL_PUNCH_HOLE or FALLOC_FL_ZERO_RANGE, makes the range within the file a hole
 * first. Blocks never written read as zeros, and take no space until written: an extent store
 * can't reserve them, so a write to them can still find no space, though more than the filesystem
 * has room for is refused. Other modes aren't supported.
 */
long fxfs_data_fallocate(struct file *file, int mode, loff_t offset, loff_t len)
{
	struct inode *inode = file_inode(file);
	loff_t end = offset + len;
	long err;

	if (mode & ~(FALLOC_FL_KEEP_SIZE | FALLOC_FL_PUNCH_HOLE | FALLOC_FL_ZERO_RANGE))
		return -EOPNOTSUPP;
	if (!S_ISREG(inode->i_mode))
		return -ENODEV;
	inode_lock(inode);
	err = inode_newsize_ok(inode, end);
	/* fxdax: whatever it changes: setuid goes, as for any write (xfstests generic/683). */
	if (!err)
		err = file_modified(file);
	if (!err && (mode & (FALLOC_FL_PUNCH_HOLE | FALLOC_FL_ZERO_RANGE)))
		err = fxfs_data_punch(inode, offset, min(end, i_size_read(inode)));
	/* fxdax: what it can't reserve it can refuse (xfstests generic/213). */
	if (!err && !(mode & (FALLOC_FL_PUNCH_HOLE | FALLOC_FL_ZERO_RANGE))) {
		struct fxfs_statfs statfs;

		err = fxfs_nofs(fxfs_statfs(fxfs_vfs_fs(inode->i_sb), &statfs));
		if (!err && len > (statfs.free_blocks * statfs.block_size) + inode_get_bytes(inode))
			err = -ENOSPC;
	}
	if (!err && !(mode & FALLOC_FL_KEEP_SIZE) && end > i_size_read(inode))
		err = fxfs_data_resize(inode, end, NULL, NULL);
	inode_unlock(inode);
	return err;
}

/*
 * FICLONE, FICLONERANGE and copy_file_range within the filesystem: makes `file_out`'s `len` bytes
 * at `pos_out` share `file_in`'s at `pos_in` (fxfs_clone_range), copying nothing, with both inodes'
 * locks and invalidate_locks held, after the VFS has checked and written back both ranges: a
 * clone to the source's end takes its last block whole. What holds each file's data readies it
 * first (remap_begin), and a destination's lets go of what it held of the range after (remapped).
 * Not FIDEDUPERANGE.
 */
loff_t fxfs_data_remap_file_range(struct file *file_in, loff_t pos_in, struct file *file_out,
				  loff_t pos_out, loff_t len, unsigned int remap_flags)
{
	struct inode *src = file_inode(file_in), *dst = file_inode(file_out);
	const struct fxfs_data_ops *src_ops = fxfs_data_ops(src), *dst_ops = fxfs_data_ops(dst);
	struct fxfs_attr attr;
	loff_t blocks, size;
	int err;

	if (remap_flags & ~(REMAP_FILE_DEDUP | REMAP_FILE_ADVISORY))
		return -EINVAL;
	if (remap_flags & REMAP_FILE_DEDUP)
		return -EOPNOTSUPP;
	lock_two_nondirectories(src, dst);
	filemap_invalidate_lock_two(src->i_mapping, dst->i_mapping);
	err = generic_remap_file_range_prep(file_in, pos_in, file_out, pos_out, &len, remap_flags);
	if (err < 0 || !len)
		goto out;
	blocks = round_up(len, i_blocksize(src));
	size = max(i_size_read(dst), pos_out + len);
	err = src_ops->remap_begin(src, pos_in, blocks, i_size_read(src));
	if (!err)
		err = dst_ops->remap_begin(dst, pos_out, blocks, size);
	if (err)
		goto out;
	err = fxfs_nofs(fxfs_clone_range(fxfs_vfs_fs(dst->i_sb), src->i_ino, pos_in, blocks,
					 dst->i_ino, pos_out, size, &attr));
	if (err) {
		/*
		 * With no room, what it got to stands, and the size it reached, if past the inode's:
		 * Fxfs has no more of a file than was written back, so its size may be short of the
		 * inode's, whose dirty pages past it a smaller size would lose.
		 */
		if (fxfs_nofs(fxfs_attr_of_open(fxfs_vfs_fs(dst->i_sb), dst->i_ino, &attr)))
			goto out;
		size = max_t(loff_t, i_size_read(dst), attr.size);
	}
	i_size_write(dst, size);
	dst->i_blocks = attr.blocks;
	if (dst_ops->remapped) {
		int remapped = dst_ops->remapped(dst, pos_out, blocks);

		err = err ?: remapped;
	}
out:
	filemap_invalidate_unlock_two(src->i_mapping, dst->i_mapping);
	unlock_two_nondirectories(src, dst);
	return err ?: len;
}
