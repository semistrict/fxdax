// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Fxfs's VFS glue: the kernel's superblock, inode, directory and file operations, over the
 * filesystem in Rust (fxfs.h).
 *
 * Rust's allocator may reclaim memory, which must not recurse into filesystems, so every call into
 * Rust runs in a NOFS allocation scope.
 *
 * Files' data is in the page cache, which writeback writes to Fxfs, contiguous dirty folios in
 * one write, and reads fill, readahead's folios in one read; or, for files of FXFS_DAX_MIN_SIZE
 * or more when the device's DAX window can map them, in the window (dax.c). The kernel keeps inodes' times, which write_inode writes back.
 */

#include <linux/backing-dev.h>
#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/init.h>
#include <linux/minmax.h>
#include <linux/module.h>
#include <linux/namei.h>
#include <linux/pagemap.h>
#include <linux/sched/mm.h>
#include <linux/slab.h>
#include <linux/statfs.h>
#include <linux/uio.h>
#include <linux/virtio_extent.h>
#include <linux/writeback.h>
#include <linux/xattr.h>

#include "fxfs.h"
#include "vfs.h"

#define FXFS_MAGIC 0x46784673 /* "FxFs" */

/* The most bytes a read or write moves to or from Rust at once. */
#define FXFS_IO_MAX (1 << 20)

/* The size from which a file read in maps its data from the DAX window, if there is one. */
#define FXFS_DAX_MIN_SIZE (1 << 20)


static const struct inode_operations fxfs_vfs_dir_inode_operations;
static const struct inode_operations fxfs_vfs_file_inode_operations;
static const struct inode_operations fxfs_vfs_symlink_inode_operations;
static const struct inode_operations fxfs_vfs_special_inode_operations;
static const struct file_operations fxfs_vfs_dir_operations;
static const struct file_operations fxfs_vfs_file_operations;
static const struct address_space_operations fxfs_vfs_aops;

static struct kmem_cache *fxfs_vfs_inode_cache;

/*
 * Sets `inode`'s attributes to `attr`, as Fxfs has them. A regular file's size and data's times
 * are the page cache's once its inode is read in, and Fxfs's lag them until writeback: only a new
 * inode takes them (fxfs_vfs_update_attr).
 */
static void fxfs_vfs_set_attr(struct inode *inode, const struct fxfs_attr *attr)
{
	inode->i_mode = attr->mode;
	set_nlink(inode, attr->nlink);
	i_uid_write(inode, attr->uid);
	i_gid_write(inode, attr->gid);
	i_size_write(inode, attr->size);
	inode->i_blocks = attr->blocks;
	inode_set_atime_to_ts(inode, ns_to_timespec64(attr->atime_ns));
	inode_set_mtime_to_ts(inode, ns_to_timespec64(attr->mtime_ns));
	inode_set_ctime_to_ts(inode, ns_to_timespec64(attr->ctime_ns));
}

/* The inode `attr` describes, read in if it is not cached. */
static struct inode *fxfs_vfs_iget(struct super_block *sb, const struct fxfs_attr *attr)
{
	struct inode *inode = iget_locked(sb, attr->ino);

	if (!inode)
		return ERR_PTR(-ENOMEM);
	if (!(inode->i_state & I_NEW))
		return inode;
	fxfs_vfs_set_attr(inode, attr);
	switch (attr->mode & S_IFMT) {
	case S_IFDIR:
		inode->i_op = &fxfs_vfs_dir_inode_operations;
		inode->i_fop = &fxfs_vfs_dir_operations;
		break;
	case S_IFREG:
		inode->i_op = &fxfs_vfs_file_inode_operations;
		inode->i_fop = &fxfs_vfs_file_operations;
		inode->i_mapping->a_ops = &fxfs_vfs_aops;
		/*
		 * Files large enough to share map their data from the window; others, small or new,
		 * are in the page cache, whose writes are deferred, as virtio-fs's dax=inode decides
		 * per file. A file takes its mode as its inode is read in.
		 */
		if (fxfs_vfs_info(sb)->dax && attr->size >= FXFS_DAX_MIN_SIZE)
			fxfs_dax_set_inode(inode);
		break;
	case S_IFLNK:
		inode->i_op = &fxfs_vfs_symlink_inode_operations;
		break;
	default:
		inode->i_op = &fxfs_vfs_special_inode_operations;
		init_special_inode(inode, inode->i_mode, new_decode_dev(attr->rdev));
		break;
	}
	unlock_new_inode(inode);
	return inode;
}

/*
 * Updates `inode`'s attributes after one of Fxfs's operations on names changed them: its link
 * count and change time, and a directory's modification time. The rest are the kernel's once the
 * inode is read in, which changes them (setattr) and writes them back (write_inode).
 */
static void fxfs_vfs_update_attr(struct inode *inode, const struct fxfs_attr *attr)
{
	set_nlink(inode, attr->nlink);
	inode_set_ctime_to_ts(inode, ns_to_timespec64(attr->ctime_ns));
	if (S_ISDIR(inode->i_mode))
		inode_set_mtime_to_ts(inode, ns_to_timespec64(attr->mtime_ns));
}

/*
 * Writes the attributes the kernel keeps to Fxfs: its mode, owner and times, and with
 * FXFS_SET_SIZE in `valid`, its size.
 */
static int fxfs_vfs_write_attr(struct inode *inode, u32 valid)
{
	struct timespec64 atime = inode_get_atime(inode), mtime = inode_get_mtime(inode);
	struct timespec64 ctime = inode_get_ctime(inode);
	struct fxfs_setattr set = {
		.valid = valid | FXFS_SET_MODE | FXFS_SET_UID | FXFS_SET_GID | FXFS_SET_ATIME |
			 FXFS_SET_MTIME | FXFS_SET_CTIME,
		.mode = inode->i_mode,
		.uid = i_uid_read(inode),
		.gid = i_gid_read(inode),
		.size = i_size_read(inode),
		.atime_ns = timespec64_to_ns(&atime),
		.mtime_ns = timespec64_to_ns(&mtime),
		.ctime_ns = timespec64_to_ns(&ctime),
	};

	return fxfs_nofs(fxfs_setattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
}

static struct dentry *fxfs_vfs_lookup(struct inode *dir, struct dentry *dentry,
				      unsigned int flags)
{
	struct fxfs_attr attr;
	struct inode *inode = NULL;
	int err;

	/* Which keeps names longer than Linux's out of Fxfs, which has no limit of its own. */
	if (dentry->d_name.len > NAME_MAX)
		return ERR_PTR(-ENAMETOOLONG);
	err = fxfs_nofs(fxfs_lookup(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
				    dentry->d_name.len, &attr));
	if (!err)
		inode = fxfs_vfs_iget(dir->i_sb, &attr);
	else if (err != -ENOENT)
		return ERR_PTR(err);
	return d_splice_alias(inode, dentry);
}

/*
 * Instantiates `dentry`, a new name in `dir`, with the inode `attr` describes, and `dir` as
 * `dir_attr` describes it.
 */
static int fxfs_vfs_instantiate(struct inode *dir, struct dentry *dentry,
				const struct fxfs_attr *attr, const struct fxfs_attr *dir_attr)
{
	struct inode *inode = fxfs_vfs_iget(dir->i_sb, attr);

	fxfs_vfs_update_attr(dir, dir_attr);
	if (IS_ERR(inode))
		return PTR_ERR(inode);
	d_instantiate(dentry, inode);
	return 0;
}

/*
 * The group a new inode in `dir` belongs to, as inode_init_owner has it: in a set-group-ID
 * directory, the directory's, and a new directory is set-group-ID too.
 */
static u32 fxfs_vfs_new_gid(struct inode *dir, umode_t *mode)
{
	if (!(dir->i_mode & S_ISGID))
		return from_kgid(&init_user_ns, current_fsgid());
	if (S_ISDIR(*mode))
		*mode |= S_ISGID;
	return from_kgid(&init_user_ns, dir->i_gid);
}

/* Creates `dentry` in `dir` as `mode` says, owned by the caller. */
static int fxfs_vfs_mknod(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			  umode_t mode, dev_t rdev)
{
	u32 gid = fxfs_vfs_new_gid(dir, &mode);
	struct fxfs_attr attr, dir_attr;
	int err;

	err = fxfs_nofs(fxfs_create(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
				    dentry->d_name.len, mode, new_encode_dev(rdev),
				    from_kuid(&init_user_ns, current_fsuid()), gid, &attr,
				    &dir_attr));
	if (err)
		return err;
	return fxfs_vfs_instantiate(dir, dentry, &attr, &dir_attr);
}

static int fxfs_vfs_create(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			   umode_t mode, bool excl)
{
	return fxfs_vfs_mknod(idmap, dir, dentry, mode | S_IFREG, 0);
}

static int fxfs_vfs_mkdir(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			  umode_t mode)
{
	return fxfs_vfs_mknod(idmap, dir, dentry, mode | S_IFDIR, 0);
}

static int fxfs_vfs_symlink(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			    const char *target)
{
	umode_t mode = S_IFLNK | 0777;
	u32 gid = fxfs_vfs_new_gid(dir, &mode);
	struct fxfs_attr attr, dir_attr;
	int err;

	if (strlen(target) > FXFS_SYMLINK_MAX)
		return -ENAMETOOLONG;
	err = fxfs_nofs(fxfs_symlink(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
					 dentry->d_name.len, target, strlen(target),
					 from_kuid(&init_user_ns, current_fsuid()), gid, &attr,
					 &dir_attr));

	if (err)
		return err;
	return fxfs_vfs_instantiate(dir, dentry, &attr, &dir_attr);
}

static int fxfs_vfs_link(struct dentry *old_dentry, struct inode *dir, struct dentry *dentry)
{
	struct inode *inode = d_inode(old_dentry);
	struct fxfs_attr attr, dir_attr;
	int err = fxfs_nofs(fxfs_link(fxfs_vfs_fs(dir->i_sb), inode->i_ino, dir->i_ino,
				      dentry->d_name.name, dentry->d_name.len, &attr, &dir_attr));

	if (err)
		return err;
	fxfs_vfs_update_attr(inode, &attr);
	fxfs_vfs_update_attr(dir, &dir_attr);
	ihold(inode);
	d_instantiate(dentry, inode);
	return 0;
}

/*
 * Accounts for what removing or replacing a name took from `inode`, which `replaced`
 * (FXFS_REPLACED_*) says: a link, or a directory, which is gone. Its inode, unlinked, is freed
 * once the kernel is done with it (fxfs_vfs_evict_inode).
 */
static void fxfs_vfs_removed(struct inode *inode, int replaced)
{
	if (!inode || replaced == FXFS_REPLACED_NONE)
		return;
	if (replaced == FXFS_REPLACED_DIRECTORY)
		clear_nlink(inode);
	else
		drop_nlink(inode);
	inode_set_ctime_current(inode);
	/* The ctime is Fxfs's too, for an inode other names still link (write_inode). */
	if (inode->i_nlink)
		mark_inode_dirty(inode);
}

static int fxfs_vfs_unlink(struct inode *dir, struct dentry *dentry)
{
	struct fxfs_attr dir_attr;
	int replaced = fxfs_nofs(fxfs_unlink(fxfs_vfs_fs(dir->i_sb), dir->i_ino,
					     dentry->d_name.name, dentry->d_name.len,
					     d_inode(dentry)->i_ino, &dir_attr));

	if (replaced < 0)
		return replaced;
	fxfs_vfs_removed(d_inode(dentry), replaced);
	fxfs_vfs_update_attr(dir, &dir_attr);
	return 0;
}

static int fxfs_vfs_rename(struct mnt_idmap *idmap, struct inode *old_dir,
			   struct dentry *old_dentry, struct inode *new_dir,
			   struct dentry *new_dentry, unsigned int flags)
{
	struct inode *inode = d_inode(old_dentry), *target = d_inode(new_dentry);
	struct fxfs_attr attr, new_attr, dir_attr, new_dir_attr;
	struct fxfs *fs = fxfs_vfs_fs(old_dir->i_sb);
	int replaced, err;

	if (flags & RENAME_EXCHANGE) {
		err = fxfs_nofs(fxfs_exchange(fs, old_dir->i_ino, old_dentry->d_name.name,
					      old_dentry->d_name.len, new_dir->i_ino,
					      new_dentry->d_name.name, new_dentry->d_name.len, &attr,
					      &new_attr, &dir_attr, &new_dir_attr));
		if (err)
			return err;
		fxfs_vfs_update_attr(target, &new_attr);
	} else {
		/* The VFS checks RENAME_NOREPLACE; no other flag is Fxfs's. */
		if (flags & ~RENAME_NOREPLACE)
			return -EINVAL;
		replaced = fxfs_nofs(fxfs_rename(fs, old_dir->i_ino, old_dentry->d_name.name,
						 old_dentry->d_name.len, inode->i_ino, new_dir->i_ino,
						 new_dentry->d_name.name, new_dentry->d_name.len,
						 target ? target->i_ino : 0, &attr, &dir_attr,
						 &new_dir_attr));
		if (replaced < 0)
			return replaced;
		fxfs_vfs_removed(target, replaced);
	}
	fxfs_vfs_update_attr(inode, &attr);
	fxfs_vfs_update_attr(old_dir, &dir_attr);
	fxfs_vfs_update_attr(new_dir, &new_dir_attr);
	return 0;
}

/*
 * Sets the attributes `iattr` says, as the VFS has them (setattr_copy: the current time unless the
 * caller gives one, set-group-ID cleared as it must be), on the inode, which writes them back to
 * Fxfs (write_inode), as ext4's journal does: a change of size at once.
 */
static int fxfs_vfs_setattr(struct mnt_idmap *idmap, struct dentry *dentry, struct iattr *iattr)
{
	struct inode *inode = d_inode(dentry);
	loff_t size = iattr->ia_size;
	int err = setattr_prepare(idmap, dentry, iattr);

	if (err)
		return err;
	if (!(iattr->ia_valid & ATTR_SIZE) || size == i_size_read(inode)) {
		setattr_copy(idmap, inode, iattr);
		mark_inode_dirty(inode);
		return 0;
	}
	/* truncate(2) leaves the times to the filesystem, which changing the size changes. */
	if (!(iattr->ia_valid & (ATTR_MTIME | ATTR_CTIME))) {
		iattr->ia_ctime = current_time(inode);
		iattr->ia_mtime = iattr->ia_ctime;
		iattr->ia_valid |= ATTR_MTIME | ATTR_CTIME;
	}
	if (IS_DAX(inode)) {
		err = fxfs_dax_resize_begin(inode, size);
		if (err)
			return err;
	}
	/* The page cache past the size goes first, so writeback can't write it back. */
	truncate_setsize(inode, size);
	setattr_copy(idmap, inode, iattr);
	err = fxfs_vfs_write_attr(inode, FXFS_SET_SIZE);
	if (IS_DAX(inode))
		fxfs_dax_resize_end(inode, size);
	return err;
}

static int fxfs_vfs_xattr_get(const struct xattr_handler *handler, struct dentry *dentry,
			      struct inode *inode, const char *name, void *buf, size_t size)
{
	const char *full = xattr_full_name(handler, name);

	return fxfs_nofs(fxfs_getxattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, full, strlen(full),
				       buf, size));
}

/* Sets an extended attribute, or removes it, without a value. Its ctime is Fxfs's (refresh). */
static int fxfs_vfs_xattr_set(const struct xattr_handler *handler, struct mnt_idmap *idmap,
			      struct dentry *dentry, struct inode *inode, const char *name,
			      const void *value, size_t size, int flags)
{
	const char *full = xattr_full_name(handler, name);
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	int err;

	if (value)
		err = fxfs_nofs(fxfs_setxattr(fs, inode->i_ino, full, strlen(full), value, size,
					      flags));
	else
		err = fxfs_nofs(fxfs_removexattr(fs, inode->i_ino, full, strlen(full)));
	if (err)
		return err;
	inode_set_ctime_current(inode);
	mark_inode_dirty(inode);
	return 0;
}

static const struct xattr_handler fxfs_vfs_user_xattr_handler = {
	.prefix = XATTR_USER_PREFIX,
	.get = fxfs_vfs_xattr_get,
	.set = fxfs_vfs_xattr_set,
};

static const struct xattr_handler fxfs_vfs_trusted_xattr_handler = {
	.prefix = XATTR_TRUSTED_PREFIX,
	.get = fxfs_vfs_xattr_get,
	.set = fxfs_vfs_xattr_set,
};

static const struct xattr_handler fxfs_vfs_security_xattr_handler = {
	.prefix = XATTR_SECURITY_PREFIX,
	.get = fxfs_vfs_xattr_get,
	.set = fxfs_vfs_xattr_set,
};

static const struct xattr_handler *const fxfs_vfs_xattr_handlers[] = {
	&fxfs_vfs_user_xattr_handler,
	&fxfs_vfs_trusted_xattr_handler,
	&fxfs_vfs_security_xattr_handler,
	NULL,
};

static ssize_t fxfs_vfs_listxattr(struct dentry *dentry, char *buf, size_t size)
{
	struct inode *inode = d_inode(dentry);

	return fxfs_nofs(fxfs_listxattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, buf, size));
}

static const char *fxfs_vfs_get_link(struct dentry *dentry, struct inode *inode,
				     struct delayed_call *done)
{
	ssize_t len;
	char *target;

	/* Reading the target may sleep, which a walk under RCU can't. */
	if (!dentry)
		return ERR_PTR(-ECHILD);
	target = kmalloc(PATH_MAX, GFP_KERNEL);
	if (!target)
		return ERR_PTR(-ENOMEM);
	len = fxfs_nofs(fxfs_readlink(fxfs_vfs_fs(inode->i_sb), inode->i_ino, target,
				      PATH_MAX - 1));
	if (len < 0 || len >= PATH_MAX) {
		kfree(target);
		return ERR_PTR(len < 0 ? len : -ENAMETOOLONG);
	}
	target[len] = 0;
	set_delayed_call(done, kfree_link, target);
	return target;
}

/*
 * Where reading an open directory got to: the position after the last entry it read, and that
 * entry's name, which the next read continues after, whatever was removed meanwhile.
 */
struct fxfs_vfs_dir_cursor {
	loff_t pos;
	size_t len;
	char name[NAME_MAX + 1];
};

struct fxfs_vfs_emitting {
	struct dir_context *ctx;
	struct fxfs_vfs_dir_cursor *cursor;
};

static bool fxfs_vfs_emit(void *arg, const char *name, size_t len, u64 ino, u32 mode)
{
	struct fxfs_vfs_emitting *emitting = arg;
	struct dir_context *ctx = emitting->ctx;
	struct fxfs_vfs_dir_cursor *cursor = emitting->cursor;

	if (!dir_emit(ctx, name, len, ino, fs_umode_to_dtype(mode)))
		return false;
	ctx->pos++;
	if (len <= NAME_MAX) {
		memcpy(cursor->name, name, len);
		cursor->len = len;
		cursor->pos = ctx->pos;
	}
	return true;
}

static int fxfs_vfs_dir_open(struct inode *inode, struct file *file)
{
	file->private_data = kzalloc(sizeof(struct fxfs_vfs_dir_cursor), GFP_KERNEL);
	return file->private_data ? 0 : -ENOMEM;
}

static int fxfs_vfs_dir_release(struct inode *inode, struct file *file)
{
	kfree(file->private_data);
	return 0;
}

/*
 * Positions 0 and 1 are "." and "..", and the filesystem's entries follow, in order. A read
 * continuing where the last left off continues after the last name it read, which removing
 * entries before it doesn't move; another, after a seek, counts entries.
 */
static int fxfs_vfs_iterate(struct file *file, struct dir_context *ctx)
{
	struct inode *inode = file_inode(file);
	struct fxfs_vfs_dir_cursor *cursor = file->private_data;
	struct fxfs_vfs_emitting emitting = { .ctx = ctx, .cursor = cursor };
	bool resume;

	if (!dir_emit_dots(file, ctx))
		return 0;
	resume = cursor->len && cursor->pos == ctx->pos;
	return fxfs_nofs(fxfs_readdir(fxfs_vfs_fs(inode->i_sb), inode->i_ino, ctx->pos - 2,
				      resume ? cursor->name : NULL, cursor->len, fxfs_vfs_emit,
				      &emitting));
}

/* Fills `folio` with file `inode`'s data, and zeros past its end. */
static int fxfs_vfs_fill_folio(struct inode *inode, struct folio *folio)
{
	size_t size = folio_size(folio);
	void *addr = folio_address(folio);
	ssize_t n = fxfs_nofs(fxfs_read(fxfs_vfs_fs(inode->i_sb), inode->i_ino, folio_pos(folio),
					addr, size));

	if (n < 0)
		return n;
	memset(addr + n, 0, size - n);
	return 0;
}

static int fxfs_vfs_read_folio(struct file *file, struct folio *folio)
{
	int err = fxfs_vfs_fill_folio(folio->mapping->host, folio);

	if (!err)
		folio_mark_uptodate(folio);
	folio_unlock(folio);
	return err;
}

/*
 * Reads the folios of `rac` with one read, if it can have the memory for it. Folios it doesn't
 * read are read one at a time (fxfs_vfs_read_folio).
 */
static void fxfs_vfs_readahead(struct readahead_control *rac)
{
	struct inode *inode = rac->mapping->host;
	size_t len = readahead_length(rac);
	loff_t pos = readahead_pos(rac);
	unsigned int nofs = memalloc_nofs_save();
	void *buf = len <= FXFS_IO_MAX ? kvmalloc(len, GFP_KERNEL) : NULL;
	ssize_t n = buf ? fxfs_read(fxfs_vfs_fs(inode->i_sb), inode->i_ino, pos, buf, len) : -ENOMEM;
	struct folio *folio;

	while ((folio = readahead_folio(rac))) {
		size_t at = folio_pos(folio) - pos, size = folio_size(folio);
		size_t have = n > (ssize_t)at ? min_t(size_t, n - at, size) : 0;

		if (n >= 0) {
			memcpy(folio_address(folio), buf + at, have);
			memset(folio_address(folio) + have, 0, size - have);
			folio_mark_uptodate(folio);
		}
		folio_unlock(folio);
	}
	kvfree(buf);
	memalloc_nofs_restore(nofs);
}

/* Readies the folio a write of `len` bytes at `pos` copies into, reading what it doesn't cover. */
static int fxfs_vfs_write_begin(struct file *file, struct address_space *mapping, loff_t pos,
				unsigned int len, struct folio **foliop, void **fsdata)
{
	struct inode *inode = mapping->host;
	struct folio *folio = __filemap_get_folio(mapping, pos >> PAGE_SHIFT, FGP_WRITEBEGIN,
						  mapping_gfp_mask(mapping));
	int err;

	if (IS_ERR(folio))
		return PTR_ERR(folio);
	if (!folio_test_uptodate(folio) && len != folio_size(folio)) {
		if (folio_pos(folio) >= i_size_read(inode)) {
			folio_zero_range(folio, 0, folio_size(folio));
		} else {
			err = fxfs_vfs_fill_folio(inode, folio);
			if (err) {
				folio_unlock(folio);
				folio_put(folio);
				return err;
			}
		}
		folio_mark_uptodate(folio);
	}
	*foliop = folio;
	return 0;
}

/* As libfs's simple_write_end: the folio dirty, and the file grown to hold the write. */
static int fxfs_vfs_write_end(struct file *file, struct address_space *mapping, loff_t pos,
			      unsigned int len, unsigned int copied, struct folio *folio,
			      void *fsdata)
{
	struct inode *inode = mapping->host;

	if (!folio_test_uptodate(folio)) {
		if (copied < len)
			folio_zero_range(folio, offset_in_folio(folio, pos) + copied, len - copied);
		folio_mark_uptodate(folio);
	}
	if (pos + copied > inode->i_size)
		i_size_write(inode, pos + copied);
	folio_mark_dirty(folio);
	folio_unlock(folio);
	folio_put(folio);
	return copied;
}

/* Contiguous folios under writeback, which one write to Fxfs writes. */
struct fxfs_vfs_run {
	struct inode *inode;
	/* Their data, copied, or NULL to write each folio from its own memory. */
	void *buf;
	struct folio *folios[FXFS_IO_MAX / PAGE_SIZE];
	unsigned int count;
	loff_t pos;
	size_t len;
};

/* Writes `run`'s folios, and ends their writeback. */
static int fxfs_vfs_write_run(struct fxfs_vfs_run *run)
{
	struct fxfs *fs = fxfs_vfs_fs(run->inode->i_sb);
	int err = 0;
	unsigned int i;

	if (run->count && run->len)
		err = fxfs_write(fs, run->inode->i_ino, run->pos,
				 run->buf ?: folio_address(run->folios[0]), run->len);
	for (i = 0; i < run->count; i++) {
		if (err)
			mapping_set_error(run->inode->i_mapping, err);
		folio_end_writeback(run->folios[i]);
	}
	run->count = 0;
	run->len = 0;
	return err;
}

/* Writes back dirty folios, contiguous ones up to FXFS_IO_MAX bytes in one write. */
static int fxfs_vfs_writepages(struct address_space *mapping, struct writeback_control *wbc)
{
	struct fxfs_vfs_run *run = kzalloc(sizeof(*run), GFP_NOFS);
	unsigned int nofs = memalloc_nofs_save();
	struct folio *folio = NULL;
	int err = 0, run_err;

	if (!run) {
		memalloc_nofs_restore(nofs);
		return -ENOMEM;
	}
	run->inode = mapping->host;
	run->buf = kvmalloc(FXFS_IO_MAX, GFP_KERNEL);
	while ((folio = writeback_iter(mapping, wbc, folio, &err))) {
		loff_t size = i_size_read(run->inode), pos = folio_pos(folio);
		size_t len = pos < size ? min_t(loff_t, folio_size(folio), size - pos) : 0;

		/* A folio that doesn't continue the run, or doesn't fit it, starts another. */
		if (run->count && (!run->buf || pos != run->pos + run->len ||
				   run->len + len > FXFS_IO_MAX ||
				   run->count == ARRAY_SIZE(run->folios))) {
			run_err = fxfs_vfs_write_run(run);
			err = err ?: run_err;
		}
		folio_start_writeback(folio);
		if (!run->count)
			run->pos = pos;
		if (run->buf)
			memcpy(run->buf + run->len, folio_address(folio), len);
		run->folios[run->count++] = folio;
		run->len += len;
		folio_unlock(folio);
	}
	run_err = fxfs_vfs_write_run(run);
	err = err ?: run_err;
	kvfree(run->buf);
	kfree(run);
	memalloc_nofs_restore(nofs);
	return err;
}

static const struct address_space_operations fxfs_vfs_aops = {
	.read_folio = fxfs_vfs_read_folio,
	.readahead = fxfs_vfs_readahead,
	.write_begin = fxfs_vfs_write_begin,
	.write_end = fxfs_vfs_write_end,
	.writepages = fxfs_vfs_writepages,
	.dirty_folio = filemap_dirty_folio,
};

/* Writes back the file's data and times, and makes the filesystem durable. */
static int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file_inode(file);
	int err = file_write_and_wait_range(file, start, end);

	if (!err)
		err = sync_inode_metadata(inode, 1);
	if (!err)
		err = fxfs_nofs(fxfs_sync(fxfs_vfs_fs(inode->i_sb)));
	return err;
}

static const struct inode_operations fxfs_vfs_dir_inode_operations = {
	.lookup = fxfs_vfs_lookup,
	.create = fxfs_vfs_create,
	.mkdir = fxfs_vfs_mkdir,
	.mknod = fxfs_vfs_mknod,
	.symlink = fxfs_vfs_symlink,
	.link = fxfs_vfs_link,
	.unlink = fxfs_vfs_unlink,
	.rmdir = fxfs_vfs_unlink,
	.rename = fxfs_vfs_rename,
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
};

static const struct inode_operations fxfs_vfs_file_inode_operations = {
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
};

static const struct inode_operations fxfs_vfs_symlink_inode_operations = {
	.get_link = fxfs_vfs_get_link,
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
};

static const struct inode_operations fxfs_vfs_special_inode_operations = {
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
};

static const struct file_operations fxfs_vfs_dir_operations = {
	.open = fxfs_vfs_dir_open,
	.release = fxfs_vfs_dir_release,
	.llseek = generic_file_llseek,
	.read = generic_read_dir,
	.iterate_shared = fxfs_vfs_iterate,
	.fsync = fxfs_vfs_fsync,
};

static const struct file_operations fxfs_vfs_file_operations = {
	.open = generic_file_open,
	.llseek = generic_file_llseek,
	.read_iter = generic_file_read_iter,
	.write_iter = generic_file_write_iter,
	.mmap = generic_file_mmap,
	.splice_read = filemap_splice_read,
	.splice_write = iter_file_splice_write,
	.fsync = fxfs_vfs_fsync,
};

/*
 * Closes the inode's handles in Fxfs, and frees an inode no name links; a directory is gone
 * already.
 */
static void fxfs_vfs_evict_inode(struct inode *inode)
{
	unsigned int nofs;

	truncate_inode_pages_final(&inode->i_data);
	fxfs_dax_evict(inode);
	clear_inode(inode);
	nofs = memalloc_nofs_save();
	fxfs_evict(fxfs_vfs_fs(inode->i_sb), inode->i_ino,
		   !inode->i_nlink && !S_ISDIR(inode->i_mode));
	memalloc_nofs_restore(nofs);
}

/* Writes back the attributes the kernel keeps (fxfs_vfs_write_attr). */
static int fxfs_vfs_write_inode(struct inode *inode, struct writeback_control *wbc)
{
	return fxfs_vfs_write_attr(inode, 0);
}

static int fxfs_vfs_sync_fs(struct super_block *sb, int wait)
{
	return fxfs_nofs(fxfs_sync(fxfs_vfs_fs(sb)));
}

static int fxfs_vfs_statfs(struct dentry *dentry, struct kstatfs *buf)
{
	struct fxfs_statfs statfs;
	int err = fxfs_nofs(fxfs_statfs(fxfs_vfs_fs(dentry->d_sb), &statfs));

	if (err)
		return err;
	buf->f_type = FXFS_MAGIC;
	buf->f_bsize = statfs.block_size;
	buf->f_blocks = statfs.blocks;
	buf->f_bfree = statfs.free_blocks;
	buf->f_bavail = statfs.free_blocks;
	buf->f_files = statfs.files;
	/* Objects take space as they need it: as many more as blocks are free. */
	buf->f_ffree = statfs.free_blocks;
	buf->f_namelen = NAME_MAX;
	return 0;
}

static struct inode *fxfs_vfs_alloc_inode(struct super_block *sb)
{
	struct fxfs_vfs_inode *fi = alloc_inode_sb(sb, fxfs_vfs_inode_cache, GFP_KERNEL);

	if (!fi)
		return NULL;
	fxfs_dax_inode_init(&fi->dax);
	return &fi->vfs;
}

static void fxfs_vfs_free_inode(struct inode *inode)
{
	kmem_cache_free(fxfs_vfs_inode_cache, fxfs_vfs_inode(inode));
}

static void fxfs_vfs_inode_init_once(void *fi)
{
	inode_init_once(&((struct fxfs_vfs_inode *)fi)->vfs);
}

static const struct super_operations fxfs_vfs_super_operations = {
	.alloc_inode = fxfs_vfs_alloc_inode,
	.free_inode = fxfs_vfs_free_inode,
	.statfs = fxfs_vfs_statfs,
	.evict_inode = fxfs_vfs_evict_inode,
	.write_inode = fxfs_vfs_write_inode,
	.sync_fs = fxfs_vfs_sync_fs,
};

/* Mounts the filesystem on the extent-store device the mount's source names by its tag. */
static int fxfs_vfs_fill_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_attr attr;
	struct inode *root;
	struct fxfs *fs;
	int err;

	/* Writeback for the page cache: the default backing device has none. */
	err = super_setup_bdi(sb);
	if (err)
		return err;
	err = fxfs_nofs(fxfs_mount(fc->source, strlen(fc->source),
				   (u64)totalram_pages() << PAGE_SHIFT, &fs, &attr));
	if (err)
		return err;
	fxfs_vfs_info(sb)->fs = fs;
	fxfs_vfs_info(sb)->dax = fxfs_dax_create(sb, fs);
	if (IS_ERR(fxfs_vfs_info(sb)->dax)) {
		err = PTR_ERR(fxfs_vfs_info(sb)->dax);
		fxfs_vfs_info(sb)->dax = NULL;
		return err;
	}
	sb->s_magic = FXFS_MAGIC;
	sb->s_op = &fxfs_vfs_super_operations;
	sb->s_xattr = fxfs_vfs_xattr_handlers;
	sb->s_blocksize = PAGE_SIZE;
	sb->s_blocksize_bits = PAGE_SHIFT;
	sb->s_maxbytes = MAX_LFS_FILESIZE;
	sb->s_time_gran = 1;

	root = fxfs_vfs_iget(sb, &attr);
	if (IS_ERR(root))
		return PTR_ERR(root);
	sb->s_root = d_make_root(root);
	return sb->s_root ? 0 : -ENOMEM;
}

/* Whether `sb` is on the device `fc` mounts. */
static int fxfs_vfs_test_super(struct super_block *sb, struct fs_context *fc)
{
	return fxfs_vfs_info(sb)->vext == fc->s_fs_info;
}

/* Readies a new superblock for the device `fc` mounts. */
static int fxfs_vfs_set_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_vfs_info *info = kzalloc(sizeof(*info), GFP_KERNEL);
	int err;

	if (!info)
		return -ENOMEM;
	err = set_anon_super_fc(sb, fc);
	if (err) {
		kfree(info);
		return err;
	}
	info->vext = fc->s_fs_info;
	sb->s_fs_info = info;
	return 0;
}

/*
 * One superblock per device, as get_tree_keyed has it: mounting a device mounted already shares
 * its filesystem.
 */
static int fxfs_vfs_get_tree(struct fs_context *fc)
{
	struct super_block *sb;
	int err;

	if (!fc->source)
		return invalf(fc, "fxfs: the source must be an extent store's tag");
	fc->s_fs_info = virtio_extent_find(fc->source);
	if (!fc->s_fs_info)
		return -ENOENT;
	sb = sget_fc(fc, fxfs_vfs_test_super, fxfs_vfs_set_super);
	if (IS_ERR(sb))
		return PTR_ERR(sb);
	if (!sb->s_root) {
		err = fxfs_vfs_fill_super(sb, fc);
		if (err) {
			deactivate_locked_super(sb);
			return err;
		}
		sb->s_flags |= SB_ACTIVE;
	}
	fc->root = dget(sb->s_root);
	return 0;
}

static const struct fs_context_operations fxfs_vfs_context_operations = {
	.get_tree = fxfs_vfs_get_tree,
};

static int fxfs_vfs_init_fs_context(struct fs_context *fc)
{
	fc->ops = &fxfs_vfs_context_operations;
	return 0;
}

static void fxfs_vfs_kill_sb(struct super_block *sb)
{
	struct fxfs_vfs_info *info = fxfs_vfs_info(sb);
	unsigned int nofs;

	kill_anon_super(sb);
	if (info->fs) {
		nofs = memalloc_nofs_save();
		fxfs_unmount(info->fs);
		memalloc_nofs_restore(nofs);
	}
	/* Once unmounted: the last seals call the DAX's sealer. */
	if (info->dax)
		fxfs_dax_destroy(info->dax);
	kfree(info);
}

static struct file_system_type fxfs_vfs_type = {
	.owner = THIS_MODULE,
	.name = "fxfs",
	.init_fs_context = fxfs_vfs_init_fs_context,
	.kill_sb = fxfs_vfs_kill_sb,
};

static int __init fxfs_vfs_init(void)
{
	int err;

	fxfs_vfs_inode_cache = kmem_cache_create("fxfs_inode", sizeof(struct fxfs_vfs_inode), 0,
						 SLAB_RECLAIM_ACCOUNT | SLAB_ACCOUNT,
						 fxfs_vfs_inode_init_once);
	if (!fxfs_vfs_inode_cache)
		return -ENOMEM;
	fxfs_init();
	err = register_filesystem(&fxfs_vfs_type);
	if (err)
		kmem_cache_destroy(fxfs_vfs_inode_cache);
	return err;
}

static void __exit fxfs_vfs_exit(void)
{
	unregister_filesystem(&fxfs_vfs_type);
	rcu_barrier();
	kmem_cache_destroy(fxfs_vfs_inode_cache);
}

module_init(fxfs_vfs_init);
module_exit(fxfs_vfs_exit);

MODULE_DESCRIPTION("Fxfs, in Rust");
MODULE_LICENSE("Dual BSD/GPL");
MODULE_ALIAS_FS("fxfs");
