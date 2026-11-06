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
#include <sim/shadow.h>
#include <linux/dax.h>

#include "../../linux/fs/fxfs/vfs.h"
#include "../../linux/fs/fxfs/device.h"

/* What vfs.h declares of the followers' tests: off, and followers not built. */
DEFINE_STATIC_KEY_FALSE(fxfs_vfs_followers);
unsigned int fxfs_follow_iomap_delay_us;
bool fxfs_follow_skip_barrier;

static struct mm_struct sim_mm = { .next = 1UL << 32 };

/* A file opened, as the mount keeps it: whether its inode is evicted. */
struct sim_file {
	struct file file;
	struct fxfs_vfs_inode inode;
	struct list_head list;
	bool evicted;
};

/* The mount, while there is one; whether a run made one, and whether its end was checked. */
static struct sim_mount *live;
static bool mounted, ended;

static void unended(void)
{
	if (mounted && !ended) {
		sim_log("sim: the test ended without sim_end\n");
		host_exit(2);
	}
}

/*
 * A mount that failed, as a sweep has one, having left nothing behind, which the end of the run
 * sees: the run ends there.
 */
static __noreturn void mount_failed(struct sim_mount *mount, const char *why, long err)
{
	sim_unexpected("the mount: %s: %ld", why, err);
	kfree(mount->info);
	kfree(mount);
	sim_log("sim: the mount failed: the run ends\n");
	sim_end(SIM_END_READ_FILES);
	host_end(0);
}

/* As fxfs_vfs_filesystem_open's and fxfs_vfs_fill_super's, of one volume. */
struct sim_mount *sim_mount_new(void)
{
	struct sim_mount *mount = kzalloc(sizeof(*mount), GFP_KERNEL);
	struct fxfs_vfs_info *info = kzalloc(sizeof(*info), GFP_KERNEL);
	struct fxfs *fs = fxfs_sim_new();

	if (!mounted)
		host_at_exit(unended);
	mounted = true;
	INIT_LIST_HEAD(&sim_mm.vmas);
	init_rwsem(&sim_mm.lock);
	current->mm = &sim_mm;
	INIT_LIST_HEAD(&mount->files);
	INIT_LIST_HEAD(&mount->shadows);
	mount->info = info;
	if (!fs)
		mount_failed(mount, "no device", -ENODEV);
	info->fs = fs;
	info->dax = fxfs_dax_create((struct fxfs_filesystem *)fs);
	if (IS_ERR_OR_NULL(info->dax)) {
		fxfs_sim_free(fs);
		mount_failed(mount, "fxfs_dax_create", PTR_ERR(info->dax));
	}
	info->dax_volume = fxfs_dax_add_volume(info->dax, fs);
	if (IS_ERR(info->dax_volume)) {
		fxfs_dax_destroy(info->dax);
		fxfs_sim_free(fs);
		mount_failed(mount, "fxfs_dax_add_volume", PTR_ERR(info->dax_volume));
	}
	mount->sb.s_fs_info = info;
	mount->sb.s_maxbytes = MAX_LFS_FILESIZE;
	live = mount;
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

	container_of(file, struct sim_file, file)->evicted = true;
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
 * syncfs(2) of the mount, as the VFS syncs a filesystem: copying on write, the window's dirty pages
 * written back, with the error any writeback of its files met since the last (s_wb_err). One that
 * fails has whatever changed since each file was last durable lost (kernel/shadow.c), and is tried
 * again: a failure leaves nothing the next sync can't do, but for what a sweep fails of it.
 */
static int sync_mount(struct sim_mount *mount)
{
	int err = 0, tries;

	/* The error a writeback met before, told once, and a sweep's failure: two at most. */
	for (tries = 0; tries < 3; tries++) {
		int recorded;

		err = fxfs_dax_settle(mount->info->dax);
		recorded = xchg(&mount->sb.s_wb_err, 0);
		err = err ?: recorded;
		if (!err)
			break;
		sim_log("sim: syncing the mount: %d\n", err);
		sim_shadow_lost(mount);
	}
	return err;
}

/*
 * As an unmount: every mapping of the mount's files gone, synced, as the VFS syncs a filesystem it
 * unmounts, the files' inodes evicted (evict_inodes), then fxfs_vfs_kill_sb's, of the last volume.
 */
void sim_mount_free(struct sim_mount *mount)
{
	struct fxfs_vfs_info *info = mount->info;
	struct sim_file *file, *next;
	struct vm_area_struct *vma, *after;
	int err;

	list_for_each_entry_safe(vma, after, &sim_mm.vmas, mm_list) {
		if (vma->vm_file->f_inode->i_sb == &mount->sb)
			sim_munmap(vma->vm_start);
	}
	err = sync_mount(mount);
	if (err)
		sim_panic("syncing the mount: %d", err);
	list_for_each_entry(file, &mount->files, list) {
		if (!file->evicted)
			sim_file_evict(&file->file);
	}
	fxfs_dax_quiesce(info->dax);
	fxfs_dax_remove_volume(info->dax, info->dax_volume);
	fxfs_dax_destroy(info->dax);
	fxfs_sim_free(info->fs);
	list_for_each_entry_safe(file, next, &mount->files, list) {
		list_del(&file->list);
		kfree(file);
	}
	sim_shadow_free(mount);
	kfree(info);
	if (live == mount)
		live = NULL;
	kfree(mount);
}

/* What sim_end checks of the mount left, and whether to read its files. */
static enum sim_end_files end_files;

/*
 * The end of a run's checks of a mount left: synced, each file holding what it was told, and read
 * whole as Fxfs's extents hold it, as long as Fxfs says, then unmounted. What it reads isn't a call
 * a sweep fails: the sync, the fsyncs, the evictions and the unmount are.
 */
static void end_mount(void *arg)
{
	struct sim_mount *mount = arg;
	struct fxfs *fs = mount->info->fs;
	struct sim_file *file;
	loff_t told;
	int err;

	/* A task spawned by the test's main, which has no mm. */
	current->mm = &sim_mm;
	err = sync_mount(mount);
	if (err)
		sim_panic("the end: syncing the mount: %d", err);
	/* Each file's writeback error no fsync has told yet, its fsync's, which the next may not have. */
	list_for_each_entry(file, &mount->files, list) {
		if (file->evicted || !file->inode.vfs.i_data.wb_err)
			continue;
		err = sim_fsync(&file->file);
		err = err ? sim_fsync(&file->file) : 0;
		if (err)
			sim_panic("the end: ino %lu's fsync after one that failed: %d",
				  file->inode.vfs.i_ino, err);
	}
	sim_shadow_check_all(mount, "the end's sync");
	sim_sweep_pause(true);
	list_for_each_entry(file, &mount->files, list) {
		struct inode *inode = &file->inode.vfs;
		u64 size = fxfs_sim_size(fs, inode->i_ino);
		char *got, *held;
		ssize_t done;
		loff_t at;

		/* A writeback that failed leaves the window holding what Fxfs doesn't, as a page cache. */
		if (file->evicted || end_files == SIM_END_FILES_READ ||
		    sim_shadow_failed(mount, inode->i_ino))
			continue;
		if (i_size_read(inode) != size)
			sim_panic("the end: ino %lu is %lld bytes long, and %llu in Fxfs", inode->i_ino,
				  i_size_read(inode), size);
		if (sim_shadow_size(mount, inode->i_ino, &told) && told != size)
			sim_panic("the end: ino %lu is %llu bytes long, where it was told %lld",
				  inode->i_ino, size, told);
		got = kmalloc(size + 1, GFP_KERNEL);
		held = kmalloc(size + 1, GFP_KERNEL);
		done = sim_read(&file->file, 0, got, size);
		/*
		 * A chunk's whole range is at a chunk's alignment, which UNMAPs that failed may have
		 * taken all of, the window's room for good (TASK-44.22).
		 */
		if (done == -ENOSPC && sim_device_kept()) {
			sim_log("sim: the end: ino %lu can't be read: the window's aligned room is lost to UNMAPs that failed\n",
				inode->i_ino);
			kfree(got);
			kfree(held);
			continue;
		}
		if (done != size)
			sim_panic("the end: reading ino %lu whole: %zd of %llu", inode->i_ino, done, size);
		fxfs_sim_read(fs, inode->i_ino, 0, held, size);
		for (at = 0; at < size; at++) {
			if (got[at] != held[at])
				sim_panic("the end: ino %lu at %lld: read(2) has %#x, and Fxfs's extents %#x",
					  inode->i_ino, at, (u8)got[at], (u8)held[at]);
		}
		kfree(got);
		kfree(held);
	}
	sim_sweep_pause(false);
	sim_mount_free(mount);
}

void sim_end_void(void)
{
	ended = true;
}

void sim_end(enum sim_end_files files)
{
	if (ended)
		sim_panic("sim_end, again");
	end_files = files;
	if (live) {
		sim_spawn("end", end_mount, live);
		sim_run();
	}
	sim_device_check_unmounted();
	ended = true;
	sim_log("sim: end: %llu calls may fail\n", sim_sweep_calls());
}

/* A new inode read in for file `ino` of the mount, opened. */
static struct file *file_of(struct sim_mount *mount, u64 ino)
{
	struct sim_file *opened = kzalloc(sizeof(*opened), GFP_KERNEL);
	struct fxfs_vfs_inode *fi = &opened->inode;
	struct file *file = &opened->file;
	struct inode *inode = &fi->vfs;

	list_add_tail(&opened->list, &mount->files);
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
	if (!fxfs_dax_set_inode(inode, false) && sim_swept()) {
		sim_log("sim: sweep: ino %llu stays in the page cache, which the sim has no model of: read in again\n",
			ino);
		fxfs_dax_set_inode(inode, false);
	}
	if (!IS_DAX(inode))
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
	u64 ino = fxfs_sim_new_file(mount->info->fs);

	sim_shadow_new(mount, ino);
	return file_of(mount, ino);
}

struct file *sim_file_read_in(struct sim_mount *mount, struct file *file)
{
	struct file *again;
	u64 ino = file_inode(file)->i_ino;
	int err = fxfs_dax_write_back(file_inode(file));

	if (err)
		sim_unexpected("writing ino %llu back: %d", ino, err);
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

/* Each a call the file model is told of (kernel/shadow.c). */
ssize_t sim_write(struct file *file, loff_t pos, const void *buf, size_t len)
{
	struct iov_iter from = { .count = len, .kbuf = (char *)buf };
	struct sim_shadow_call call;
	ssize_t done;

	sim_shadow_write_begin(&call, file, pos, len);
	done = do_write(file, pos, &from);
	sim_shadow_write_end(&call, buf, done);
	return done;
}

ssize_t sim_read(struct file *file, loff_t pos, void *buf, size_t len)
{
	struct iov_iter to = { .count = len, .kbuf = buf };
	struct sim_shadow_call call;
	ssize_t done;

	sim_shadow_read_begin(&call, file, pos, len);
	done = do_read(file, pos, &to);
	sim_shadow_read_end(&call, "a read", buf, done);
	return done;
}

/* What a write from a task's memory wrote, the file model doesn't know. */
ssize_t sim_write_user(struct file *file, loff_t pos, unsigned long addr, size_t len)
{
	struct iov_iter from = { .count = len, .user = true, .uaddr = addr };
	struct sim_shadow_call call;
	ssize_t done;

	sim_shadow_write_begin(&call, file, pos, len);
	done = do_write(file, pos, &from);
	sim_shadow_write_end(&call, NULL, done);
	return done;
}

/* Nor what a read into a task's memory stored there, of a mapping. */
ssize_t sim_read_user(struct file *file, loff_t pos, unsigned long addr, size_t len)
{
	struct iov_iter to = { .count = len, .user = true, .uaddr = addr };
	struct sim_user_access access;
	ssize_t done;

	sim_user_access_begin(&access, addr, len, true);
	done = do_read(file, pos, &to);
	sim_user_access_end(&access, NULL, max_t(ssize_t, done, 0));
	return done;
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
	struct sim_shadow_call call;
	int err;

	sim_shadow_sync_begin(&call, file);
	err = file->f_inode->i_fop->fsync(file, 0, LLONG_MAX, 0);
	sim_shadow_sync_end(&call, fxfs_vfs_fs(file_inode(file)->i_sb), err);
	return err;
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
	struct sim_shadow_call call;
	int err;

	sim_shadow_resize_begin(&call, file, size);
	inode_lock(inode);
	err = fxfs_data_resize(inode, size, NULL, NULL);
	inode_unlock(inode);
	sim_shadow_resize_end(&call, err);
	return err;
}

/* FICLONERANGE, as ioctl_file_clone has it: the file operation, which takes the locks. */
int sim_clone(struct file *in, loff_t pos_in, struct file *out, loff_t pos_out, loff_t len)
{
	struct sim_shadow_clone clone;
	loff_t done;
	int err;

	sim_shadow_clone_begin(&clone, in, pos_in, out, pos_out, len);
	done = out->f_inode->i_fop->remap_file_range(in, pos_in, out, pos_out, len, 0);
	err = done < 0 ? done : done == len ? 0 : -EINVAL;
	sim_shadow_clone_end(&clone, err);
	return err;
}

/* fallocate(2) with FALLOC_FL_PUNCH_HOLE, which the VFS has keep the size. */
int sim_punch(struct file *file, loff_t start, loff_t end)
{
	struct sim_shadow_call call;
	int err;

	sim_shadow_punch_begin(&call, file, start, end);
	err = file->f_inode->i_fop->fallocate(file, FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE,
					      start, end - start);
	sim_shadow_punch_end(&call, err);
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
