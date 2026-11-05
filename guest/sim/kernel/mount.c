// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * A mounted file, as fs/fxfs/super.c makes one, in the order super.c calls dax.c: the VFS's system
 * calls for tests. super.c itself isn't built: its fsync, and its writing of an inode's attributes,
 * are rewritten here. Its changes of a file as a whole are data.c's, built as they are: a truncate,
 * a punch and a clone go there, as the VFS's system calls do.
 */

#include <sim/sim.h>
#include <sim/host.h>
#include <linux/dax.h>

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
	init_rwsem(&sim_mm.lock);
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

/*
 * fxfs_vfs_evict_inode's, for a file nothing maps: its DAX entries go, as truncate_inode_pages_final
 * has them, and the page's stale fields with them (dax_folio_put), but its ranges stay. As the
 * kernel's eviction, at the last reference: none taken from now (igrab), it waits for those taken
 * already, as reclaim's and the budget's worker's (igrab). Evicted under them, as it was, it read
 * in another inode whose faults mapped the ranges, which reclaim then released through the old
 * one's mapping alone, and freed under the new one's DAX entries (interleave's seed 222 at 1,000
 * operations: `fs/dax.c`'s WARN as another file's fault found the page associated).
 */
void sim_file_evict(struct file *file)
{
	struct inode *inode = file_inode(file);
	unsigned long index;
	void *entry;

	inode->i_state |= I_FREEING;
	while (atomic_read(&inode->i_count) > 1)
		msleep(1);
	dax_break_layout_final(inode);
	xa_for_each(&inode->i_mapping->i_pages, index, entry) {
		if (xa_is_value(entry))
			dax_delete_mapping_entry(inode->i_mapping, index);
	}
	fxfs_dax_evict(inode);
}

/*
 * As fxfs_vfs_kill_sb's, of the last volume, its files evicted: synced first, as the VFS syncs a
 * filesystem it unmounts, copying on write its window's dirty pages written back.
 */
void sim_mount_free(struct sim_mount *mount)
{
	struct fxfs_vfs_info *info = mount->info;
	int err = fxfs_dax_settle(info->dax);

	if (err)
		sim_panic("syncing the mount: %d", err);
	fxfs_dax_quiesce(info->dax);
	fxfs_dax_remove_volume(info->dax, info->dax_volume);
	fxfs_dax_destroy(info->dax);
	fxfs_sim_free(info->fs);
	kfree(info);
	kfree(mount);
}

/* A new inode read in for file `ino` of the mount, opened. */
static struct file *file_of(struct sim_mount *mount, u64 ino)
{
	struct fxfs_vfs_inode *fi = kzalloc(sizeof(*fi), GFP_KERNEL);
	struct file *file = kzalloc(sizeof(*file), GFP_KERNEL);
	struct inode *inode = &fi->vfs;

	inode->i_mode = S_IFREG | 0644;
	inode->i_nlink = 1;
	inode->i_blkbits = PAGE_SHIFT;
	inode->i_ino = ino;
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
	/*
	 * As read in, whether new or not: created, copying on write, it would be a page-cache inode
	 * (TASK-164.3), which the sim doesn't build.
	 */
	if (!fxfs_dax_set_inode(inode, false) || !IS_DAX(inode))
		sim_panic("the file isn't a DAX file");
	file->f_inode = inode;
	file->f_mapping = inode->i_mapping;
	return file;
}

u64 sim_dax_stat(struct sim_mount *mount, const char *name)
{
	struct seq_file m = { .size = 64 << 10 };
	size_t len = strlen(name);
	unsigned long long value;
	char *line;

	m.buf = kzalloc(m.size, GFP_KERNEL);
	fxfs_dax_show_stats(mount->info->dax, &m);
	for (line = m.buf; *line; line++) {
		if ((line == m.buf || line[-1] == '\n') && !strncmp(line, name, len) &&
		    line[len] == ' ' && sscanf(line + len + 1, "%llu", &value) == 1) {
			kfree(m.buf);
			return value;
		}
	}
	sim_panic("dax.c shows no %s", name);
}

struct file *sim_file_new(struct sim_mount *mount)
{
	return file_of(mount, fxfs_sim_new_file(mount->info->fs));
}

struct file *sim_file_read_in(struct sim_mount *mount, struct file *file)
{
	struct file *again;
	u64 ino = file_inode(file)->i_ino;
	int err = fxfs_dax_write_back(file_inode(file));

	if (err)
		sim_panic("writing ino %llu back: %d", ino, err);
	sim_file_evict(file);
	again = file_of(mount, ino);
	i_size_write(file_inode(again), fxfs_sim_size(mount->info->fs, ino));
	return again;
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

/*
 * super.c's, which syncs the filesystem, as a write found it full: the window's dirty pages
 * written back, copying on write, which frees the room reserved beyond what writeback took, and
 * sealed.
 */
void fxfs_vfs_make_room(struct super_block *sb)
{
	struct fxfs_vfs_info *info = fxfs_vfs_info(sb);

	if (!fxfs_dax_settle(info->dax))
		fxfs_nofs(fxfs_fsync(info->fs));
}

/* fxfs_vfs_fsync, a DAX file's: file_write_and_wait_range, its dirty pages, and the seal. */
int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file_inode(file);
	int err = filemap_write_and_wait_range(inode->i_mapping, start, end);

	if (!err)
		err = fxfs_dax_write_back(inode);
	if (!err)
		err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
	return err;
}

int sim_fsync(struct file *file)
{
	return file->f_inode->i_fop->fsync(file, 0, LLONG_MAX, 0);
}

/*
 * super.c's, which writes the inode's mode, owner and times too: here, the size, if `valid` says
 * so, as Fxfs's write_inode takes it.
 */
int fxfs_vfs_write_attr(struct inode *inode, u32 valid)
{
	struct fxfs_setattr set = {
		.valid = valid & (FXFS_SET_SIZE | FXFS_SET_KEEP_TAIL),
		.size = i_size_read(inode),
	};

	return fxfs_nofs(fxfs_write_inode(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
}

/* truncate(2), as do_truncate has it: the inode's lock, then setattr's change of size. */
int sim_truncate(struct file *file, loff_t size)
{
	struct inode *inode = file_inode(file);
	int err;

	inode_lock(inode);
	err = fxfs_data_resize(inode, size, NULL, NULL);
	inode_unlock(inode);
	return err;
}

/* FICLONERANGE, as ioctl_file_clone has it: the file operation, which takes the locks. */
int sim_clone(struct file *in, loff_t pos_in, struct file *out, loff_t pos_out, loff_t len)
{
	loff_t done = out->f_inode->i_fop->remap_file_range(in, pos_in, out, pos_out, len, 0);

	if (done < 0)
		return done;
	return done == len ? 0 : -EINVAL;
}

/* fallocate(2) with FALLOC_FL_PUNCH_HOLE, which the VFS has keep the size. */
int sim_punch(struct file *file, loff_t start, loff_t end)
{
	return file->f_inode->i_fop->fallocate(file, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
					       start, end - start);
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
