// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Fxfs's VFS glue: the kernel's superblock, inode, directory and file operations, over the
 * filesystem in Rust (fxfs.h).
 *
 * Rust's allocator may reclaim memory, which must not recurse into filesystems, so every call into
 * Rust runs in a NOFS allocation scope.
 *
 * Files' data has no page cache yet: reads and writes go to Rust a chunk at a time, and files
 * can't be mapped.
 */

#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/init.h>
#include <linux/minmax.h>
#include <linux/module.h>
#include <linux/namei.h>
#include <linux/sched/mm.h>
#include <linux/slab.h>
#include <linux/statfs.h>
#include <linux/uio.h>
#include <linux/virtio_extent.h>

#include "fxfs.h"

#define FXFS_MAGIC 0x46784673 /* "FxFs" */

/* The most bytes a read or write moves to or from Rust at once. */
#define FXFS_IO_MAX (1 << 20)

/* Evaluates `call`, a call into Rust, in a NOFS allocation scope. */
#define fxfs_nofs(call)                                        \
	({                                                     \
		unsigned int __nofs = memalloc_nofs_save();    \
		typeof(call) __ret = (call);                   \
		memalloc_nofs_restore(__nofs);                 \
		__ret;                                         \
	})

static const struct inode_operations fxfs_vfs_dir_inode_operations;
static const struct inode_operations fxfs_vfs_file_inode_operations;
static const struct inode_operations fxfs_vfs_symlink_inode_operations;
static const struct inode_operations fxfs_vfs_special_inode_operations;
static const struct file_operations fxfs_vfs_dir_operations;
static const struct file_operations fxfs_vfs_file_operations;

/* A superblock's: the device it is on, and the filesystem, once mounted. */
struct fxfs_vfs_info {
	struct virtio_extent *vext;
	struct fxfs *fs;
};

static struct fxfs_vfs_info *fxfs_vfs_info(struct super_block *sb)
{
	return sb->s_fs_info;
}

static struct fxfs *fxfs_vfs_fs(struct super_block *sb)
{
	return fxfs_vfs_info(sb)->fs;
}

/* Sets `inode`'s attributes to `attr`. */
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

/* Reads `inode`'s attributes again, after a change to them. */
static int fxfs_vfs_refresh(struct inode *inode)
{
	struct fxfs_attr attr;
	int err = fxfs_nofs(fxfs_getattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &attr));

	if (!err)
		fxfs_vfs_set_attr(inode, &attr);
	return err;
}

static struct dentry *fxfs_vfs_lookup(struct inode *dir, struct dentry *dentry,
				      unsigned int flags)
{
	struct fxfs_attr attr;
	struct inode *inode = NULL;
	int err = fxfs_nofs(fxfs_lookup(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
					dentry->d_name.len, &attr));

	if (!err)
		inode = fxfs_vfs_iget(dir->i_sb, &attr);
	else if (err != -ENOENT)
		return ERR_PTR(err);
	return d_splice_alias(inode, dentry);
}

/* Instantiates `dentry`, a new name in `dir`, with the inode `attr` describes. */
static int fxfs_vfs_instantiate(struct inode *dir, struct dentry *dentry,
				const struct fxfs_attr *attr)
{
	struct inode *inode = fxfs_vfs_iget(dir->i_sb, attr);

	if (IS_ERR(inode))
		return PTR_ERR(inode);
	d_instantiate(dentry, inode);
	return fxfs_vfs_refresh(dir);
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
	struct fxfs_attr attr;
	int err;

	err = fxfs_nofs(fxfs_create(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
				    dentry->d_name.len, mode, new_encode_dev(rdev),
				    from_kuid(&init_user_ns, current_fsuid()), gid, &attr));
	if (err)
		return err;
	return fxfs_vfs_instantiate(dir, dentry, &attr);
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
	struct fxfs_attr attr;
	int err = fxfs_nofs(fxfs_symlink(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
					 dentry->d_name.len, target, strlen(target),
					 from_kuid(&init_user_ns, current_fsuid()), gid, &attr));

	if (err)
		return err;
	return fxfs_vfs_instantiate(dir, dentry, &attr);
}

static int fxfs_vfs_link(struct dentry *old_dentry, struct inode *dir, struct dentry *dentry)
{
	struct inode *inode = d_inode(old_dentry);
	struct fxfs_attr attr;
	int err = fxfs_nofs(fxfs_link(fxfs_vfs_fs(dir->i_sb), inode->i_ino, dir->i_ino,
				      dentry->d_name.name, dentry->d_name.len, &attr));

	if (err)
		return err;
	fxfs_vfs_set_attr(inode, &attr);
	ihold(inode);
	d_instantiate(dentry, inode);
	return fxfs_vfs_refresh(dir);
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
}

static int fxfs_vfs_unlink(struct inode *dir, struct dentry *dentry)
{
	int replaced = fxfs_nofs(fxfs_unlink(fxfs_vfs_fs(dir->i_sb), dir->i_ino,
					     dentry->d_name.name, dentry->d_name.len));

	if (replaced < 0)
		return replaced;
	fxfs_vfs_removed(d_inode(dentry), replaced);
	return fxfs_vfs_refresh(dir);
}

static int fxfs_vfs_rename(struct mnt_idmap *idmap, struct inode *old_dir,
			   struct dentry *old_dentry, struct inode *new_dir,
			   struct dentry *new_dentry, unsigned int flags)
{
	int replaced, err;

	/* The VFS checks RENAME_NOREPLACE; Fxfs can't exchange. */
	if (flags & ~RENAME_NOREPLACE)
		return -EINVAL;
	replaced = fxfs_nofs(fxfs_rename(fxfs_vfs_fs(old_dir->i_sb), old_dir->i_ino,
					 old_dentry->d_name.name, old_dentry->d_name.len,
					 new_dir->i_ino, new_dentry->d_name.name,
					 new_dentry->d_name.len));
	if (replaced < 0)
		return replaced;
	fxfs_vfs_removed(d_inode(new_dentry), replaced);
	err = fxfs_vfs_refresh(d_inode(old_dentry));
	if (!err)
		err = fxfs_vfs_refresh(old_dir);
	if (!err && new_dir != old_dir)
		err = fxfs_vfs_refresh(new_dir);
	return err;
}

static int fxfs_vfs_setattr(struct mnt_idmap *idmap, struct dentry *dentry, struct iattr *iattr)
{
	struct inode *inode = d_inode(dentry);
	struct fxfs_setattr set = { 0 };
	struct fxfs_attr attr;
	int err = setattr_prepare(idmap, dentry, iattr);

	if (err)
		return err;
	if (iattr->ia_valid & ATTR_MODE) {
		set.valid |= FXFS_SET_MODE;
		set.mode = iattr->ia_mode;
	}
	if (iattr->ia_valid & ATTR_UID) {
		set.valid |= FXFS_SET_UID;
		set.uid = from_kuid(&init_user_ns, iattr->ia_uid);
	}
	if (iattr->ia_valid & ATTR_GID) {
		set.valid |= FXFS_SET_GID;
		set.gid = from_kgid(&init_user_ns, iattr->ia_gid);
	}
	if (iattr->ia_valid & ATTR_SIZE) {
		set.valid |= FXFS_SET_SIZE;
		set.size = iattr->ia_size;
	}
	/* The VFS fills in the current time unless the caller gives one. */
	if (iattr->ia_valid & ATTR_ATIME) {
		set.valid |= FXFS_SET_ATIME;
		set.atime_ns = timespec64_to_ns(&iattr->ia_atime);
	}
	if (iattr->ia_valid & ATTR_MTIME) {
		set.valid |= FXFS_SET_MTIME;
		set.mtime_ns = timespec64_to_ns(&iattr->ia_mtime);
	}
	err = fxfs_nofs(fxfs_setattr(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set, &attr));
	if (err)
		return err;
	fxfs_vfs_set_attr(inode, &attr);
	return 0;
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

static bool fxfs_vfs_emit(void *arg, const char *name, size_t len, u64 ino, u32 mode)
{
	struct dir_context *ctx = arg;

	if (!dir_emit(ctx, name, len, ino, fs_umode_to_dtype(mode)))
		return false;
	ctx->pos++;
	return true;
}

static int fxfs_vfs_iterate(struct file *file, struct dir_context *ctx)
{
	struct inode *inode = file_inode(file);

	/* Positions 0 and 1 are "." and "..", and the filesystem's entries follow. */
	if (!dir_emit_dots(file, ctx))
		return 0;
	return fxfs_nofs(fxfs_readdir(fxfs_vfs_fs(inode->i_sb), inode->i_ino, ctx->pos - 2,
				      fxfs_vfs_emit, ctx));
}

static ssize_t fxfs_vfs_read_iter(struct kiocb *iocb, struct iov_iter *to)
{
	struct inode *inode = file_inode(iocb->ki_filp);
	size_t count = iov_iter_count(to);
	ssize_t total = 0;
	void *buf;

	if (!count)
		return 0;
	buf = kvmalloc(min_t(size_t, count, FXFS_IO_MAX), GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	while (iov_iter_count(to)) {
		size_t chunk = min_t(size_t, iov_iter_count(to), FXFS_IO_MAX);
		ssize_t n = fxfs_nofs(fxfs_read(fxfs_vfs_fs(inode->i_sb), inode->i_ino,
						iocb->ki_pos, buf, chunk));

		if (n <= 0) {
			if (!total)
				total = n;
			break;
		}
		if (copy_to_iter(buf, n, to) != n) {
			if (!total)
				total = -EFAULT;
			break;
		}
		iocb->ki_pos += n;
		total += n;
		if (n < chunk)
			break;
	}
	kvfree(buf);
	return total;
}

static ssize_t fxfs_vfs_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	struct file *file = iocb->ki_filp;
	struct inode *inode = file_inode(file);
	struct fxfs_attr attr;
	ssize_t total = 0, ret;
	void *buf;

	inode_lock(inode);
	ret = generic_write_checks(iocb, from);
	if (ret <= 0)
		goto out;
	ret = file_remove_privs(file);
	if (ret)
		goto out;
	buf = kvmalloc(min_t(size_t, iov_iter_count(from), FXFS_IO_MAX), GFP_KERNEL);
	if (!buf) {
		ret = -ENOMEM;
		goto out;
	}
	while (iov_iter_count(from)) {
		size_t chunk = min_t(size_t, iov_iter_count(from), FXFS_IO_MAX);
		size_t copied = copy_from_iter(buf, chunk, from);

		if (!copied) {
			ret = -EFAULT;
			break;
		}
		ret = fxfs_nofs(fxfs_write(fxfs_vfs_fs(inode->i_sb), inode->i_ino, iocb->ki_pos,
					   buf, copied, &attr));
		if (ret)
			break;
		fxfs_vfs_set_attr(inode, &attr);
		iocb->ki_pos += copied;
		total += copied;
	}
	kvfree(buf);
	if (total)
		ret = total;
out:
	inode_unlock(inode);
	if (ret > 0)
		ret = generic_write_sync(iocb, ret);
	return ret;
}

static int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	return fxfs_nofs(fxfs_sync(fxfs_vfs_fs(file_inode(file)->i_sb)));
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
};

static const struct inode_operations fxfs_vfs_file_inode_operations = {
	.setattr = fxfs_vfs_setattr,
};

static const struct inode_operations fxfs_vfs_symlink_inode_operations = {
	.get_link = fxfs_vfs_get_link,
	.setattr = fxfs_vfs_setattr,
};

static const struct inode_operations fxfs_vfs_special_inode_operations = {
	.setattr = fxfs_vfs_setattr,
};

static const struct file_operations fxfs_vfs_dir_operations = {
	.llseek = generic_file_llseek,
	.read = generic_read_dir,
	.iterate_shared = fxfs_vfs_iterate,
	.fsync = fxfs_vfs_fsync,
};

static const struct file_operations fxfs_vfs_file_operations = {
	.open = generic_file_open,
	.llseek = generic_file_llseek,
	.read_iter = fxfs_vfs_read_iter,
	.write_iter = fxfs_vfs_write_iter,
	.fsync = fxfs_vfs_fsync,
};

/* Frees an inode no name links once the kernel is done with it; a directory is gone already. */
static void fxfs_vfs_evict_inode(struct inode *inode)
{
	unsigned int nofs;

	truncate_inode_pages_final(&inode->i_data);
	clear_inode(inode);
	if (!inode->i_nlink && !S_ISDIR(inode->i_mode)) {
		nofs = memalloc_nofs_save();
		fxfs_evict(fxfs_vfs_fs(inode->i_sb), inode->i_ino);
		memalloc_nofs_restore(nofs);
	}
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

static const struct super_operations fxfs_vfs_super_operations = {
	.statfs = fxfs_vfs_statfs,
	.drop_inode = generic_delete_inode,
	.evict_inode = fxfs_vfs_evict_inode,
	.sync_fs = fxfs_vfs_sync_fs,
};

/* Mounts the filesystem on the extent-store device the mount's source names by its tag. */
static int fxfs_vfs_fill_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_attr attr;
	struct inode *root;
	struct fxfs *fs;
	int err;

	err = fxfs_nofs(fxfs_mount(fc->source, strlen(fc->source), &fs, &attr));
	if (err)
		return err;
	fxfs_vfs_info(sb)->fs = fs;
	sb->s_magic = FXFS_MAGIC;
	sb->s_op = &fxfs_vfs_super_operations;
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
	fxfs_init();
	return register_filesystem(&fxfs_vfs_type);
}

static void __exit fxfs_vfs_exit(void)
{
	unregister_filesystem(&fxfs_vfs_type);
}

module_init(fxfs_vfs_init);
module_exit(fxfs_vfs_exit);

MODULE_DESCRIPTION("Fxfs, in Rust");
MODULE_LICENSE("Dual BSD/GPL");
MODULE_ALIAS_FS("fxfs");
