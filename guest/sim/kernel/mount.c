// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A mounted file, as fs/fxfs/super.c makes one, in the order super.c calls dax.c: the VFS's system
 * calls for tests. super.c itself isn't built: its truncate and fsync are rewritten here.
 */

#include <sim/sim.h>
#include <sim/host.h>

#include "../../linux/fs/fxfs/vfs.h"
#include "../../linux/fs/fxfs/device.h"

/* What vfs.h declares of the followers' tests: off, and followers not built. */
DEFINE_STATIC_KEY_FALSE(fxfs_vfs_followers);
unsigned int fxfs_follow_iomap_delay_us;
bool fxfs_follow_skip_barrier;

static struct mm_struct sim_mm = { .next = 1UL << 32 };

struct sim_mount *sim_mount_new(void)
{
	struct sim_mount *mount = kzalloc(sizeof(*mount), GFP_KERNEL);
	struct fxfs_vfs_info *info = kzalloc(sizeof(*info), GFP_KERNEL);
	struct fxfs *fs = fxfs_sim_new();

	INIT_LIST_HEAD(&sim_mm.vmas);
	current->mm = &sim_mm;
	info->fs = fs;
	info->dax = fxfs_dax_create((struct fxfs_filesystem *)fs);
	if (IS_ERR_OR_NULL(info->dax))
		sim_panic("fxfs_dax_create: %ld", PTR_ERR(info->dax));
	info->dax_volume = fxfs_dax_add_volume(info->dax, fs);
	mount->sb.s_fs_info = info;
	mount->sb.s_maxbytes = MAX_LFS_FILESIZE;
	mount->info = info;
	return mount;
}

void sim_file_evict(struct file *file)
{
	fxfs_dax_evict(file_inode(file));
}

/* As fxfs_vfs_kill_sb's, of the last volume, its files evicted. */
void sim_mount_free(struct sim_mount *mount)
{
	struct fxfs_vfs_info *info = mount->info;

	fxfs_dax_quiesce(info->dax);
	fxfs_dax_remove_volume(info->dax, info->dax_volume);
	fxfs_dax_destroy(info->dax);
	fxfs_sim_free(info->fs);
	kfree(info);
	kfree(mount);
}

struct file *sim_file_new(struct sim_mount *mount)
{
	struct fxfs_vfs_inode *fi = kzalloc(sizeof(*fi), GFP_KERNEL);
	struct file *file = kzalloc(sizeof(*file), GFP_KERNEL);
	struct inode *inode = &fi->vfs;

	inode->i_mode = S_IFREG | 0644;
	inode->i_nlink = 1;
	inode->i_blkbits = PAGE_SHIFT;
	inode->i_ino = fxfs_sim_new_file(mount->info->fs);
	inode->i_sb = &mount->sb;
	inode->i_mapping = &inode->i_data;
	atomic_set(&inode->i_count, 1);
	init_rwsem(&inode->i_rwsem);
	inode->i_data.host = inode;
	xa_init_flags(&inode->i_data.i_pages, XA_FLAGS_LOCK_IRQ);
	init_rwsem(&inode->i_data.invalidate_lock);
	init_rwsem(&inode->i_data.i_mmap_rwsem);
	INIT_LIST_HEAD(&inode->i_data.i_mmap);
	fxfs_dax_inode_init(&fi->dax);
	fxfs_dax_set_inode(inode);
	if (!IS_DAX(inode))
		sim_panic("the file isn't a DAX file");
	file->f_inode = inode;
	file->f_mapping = inode->i_mapping;
	return file;
}

static ssize_t do_write(struct file *file, loff_t pos, struct iov_iter *from)
{
	struct kiocb iocb = { .ki_filp = file, .ki_pos = pos };

	from->data_source = WRITE;
	return file->f_inode->i_fop->write_iter(&iocb, from);
}

static ssize_t do_read(struct file *file, loff_t pos, struct iov_iter *to)
{
	struct kiocb iocb = { .ki_filp = file, .ki_pos = pos };

	to->data_source = READ;
	return file->f_inode->i_fop->read_iter(&iocb, to);
}

ssize_t sim_write(struct file *file, loff_t pos, const void *buf, size_t len)
{
	struct iov_iter from = { .count = len, .kbuf = (char *)buf };

	return do_write(file, pos, &from);
}

ssize_t sim_read(struct file *file, loff_t pos, void *buf, size_t len)
{
	struct iov_iter to = { .count = len, .kbuf = buf };

	return do_read(file, pos, &to);
}

ssize_t sim_write_user(struct file *file, loff_t pos, unsigned long addr, size_t len)
{
	struct iov_iter from = { .count = len, .user = true, .uaddr = addr };

	return do_write(file, pos, &from);
}

ssize_t sim_read_user(struct file *file, loff_t pos, unsigned long addr, size_t len)
{
	struct iov_iter to = { .count = len, .user = true, .uaddr = addr };

	return do_read(file, pos, &to);
}

/* fxfs_vfs_fsync, a DAX file's: file_write_and_wait_range, the size, and the seal. */
int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file_inode(file);
	int err = filemap_write_and_wait_range(inode->i_mapping, start, end);

	if (!err)
		err = fxfs_dax_write_size(inode);
	if (!err)
		err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
	return err;
}

int sim_fsync(struct file *file)
{
	return file->f_inode->i_fop->fsync(file, 0, LLONG_MAX, 0);
}

/* super.c's, for a DAX file: its size written back, not the rest of the inode. */
int fxfs_vfs_resize(struct inode *inode, loff_t size, struct mnt_idmap *idmap, struct iattr *iattr)
{
	struct fxfs_setattr set = {
		.valid = FXFS_SET_SIZE | FXFS_SET_KEEP_TAIL, .size = size,
	};
	int err = fxfs_dax_resize_begin(inode, size);

	if (err)
		return err;
	truncate_setsize(inode, size);
	err = fxfs_nofs(fxfs_setattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
	fxfs_dax_resize_end(inode, size);
	return err;
}

int sim_truncate(struct file *file, loff_t size)
{
	struct inode *inode = file_inode(file);
	int err;

	inode_lock(inode);
	err = fxfs_vfs_resize(inode, size, NULL, NULL);
	inode_unlock(inode);
	return err;
}

/* fxfs_vfs_remap_file_range, for two DAX files, of whole pages. */
int sim_clone(struct file *from, struct file *to, pgoff_t page, pgoff_t pages)
{
	struct inode *src = file_inode(from), *dst = file_inode(to);
	struct address_space *in = src->i_mapping, *out = dst->i_mapping;
	loff_t pos = (loff_t)page << PAGE_SHIFT, len = (loff_t)pages << PAGE_SHIFT;
	loff_t size = max(i_size_read(dst), pos + len);
	int err;

	/* By address, as lock_two_nondirectories and filemap_invalidate_lock_two do. */
	inode_lock(min(src, dst));
	inode_lock(max(src, dst));
	filemap_invalidate_lock(min(in, out));
	filemap_invalidate_lock(max(in, out));
	err = filemap_write_and_wait_range(in, pos, pos + len - 1) ?:
	      filemap_write_and_wait_range(out, pos, pos + len - 1) ?:
	      fxfs_dax_remap(src, pos, len, i_size_read(src)) ?:
	      fxfs_dax_remap(dst, pos, len, size);
	if (!err) {
		fxfs_sim_clone(fxfs_vfs_fs(dst->i_sb), src->i_ino, dst->i_ino, page, pages, size);
		i_size_write(dst, size);
	}
	filemap_invalidate_unlock(max(in, out));
	filemap_invalidate_unlock(min(in, out));
	inode_unlock(max(src, dst));
	inode_unlock(min(src, dst));
	return err;
}

/* fxfs_vfs_punch, for a DAX file. */
int sim_punch(struct file *file, loff_t start, loff_t end)
{
	struct inode *inode = file_inode(file);
	loff_t first = round_up(start, PAGE_SIZE), last = round_down(end, PAGE_SIZE);
	int err;

	inode_lock(inode);
	err = fxfs_dax_punch_begin(inode, start, end);
	if (!err) {
		if (first < last)
			err = fxfs_nofs(fxfs_punch(fxfs_vfs_fs(inode->i_sb), inode->i_ino, first,
						   last - first));
		fxfs_dax_punch_end(inode);
	}
	inode_unlock(inode);
	return err;
}

unsigned long sim_mmap(struct file *file, loff_t offset, size_t len)
{
	return sim_mm_map(&sim_mm, file, offset >> PAGE_SHIFT, len);
}

int fxfs_vfs_file_open(struct inode *inode, struct file *file)
{
	return 0;
}

int fxfs_vfs_file_release(struct inode *inode, struct file *file)
{
	return 0;
}
