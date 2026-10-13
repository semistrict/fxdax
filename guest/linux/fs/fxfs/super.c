// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * Fxfs's VFS glue: the kernel's superblock, inode, directory and file operations, over the
 * filesystem in Rust (fxfs.h).
 *
 * Rust's allocator may reclaim memory, which must not recurse into filesystems, so every call into
 * Rust runs in a NOFS allocation scope.
 */

#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/init.h>
#include <linux/minmax.h>
#include <linux/module.h>
#include <linux/sched/mm.h>
#include <linux/slab.h>
#include <linux/uio.h>

#include "fxfs.h"

#define FXFS_MAGIC 0x46784673 /* "FxFs" */

/* Evaluates `call`, a call into Rust, in a NOFS allocation scope. */
#define fxfs_nofs(call)                                        \
	({                                                     \
		unsigned int __nofs = memalloc_nofs_save();    \
		typeof(call) __ret = (call);                   \
		memalloc_nofs_restore(__nofs);                 \
		__ret;                                         \
	})

static const struct inode_operations fxfs_vfs_dir_inode_operations;
static const struct file_operations fxfs_vfs_dir_operations;
static const struct file_operations fxfs_vfs_file_operations;

static struct fxfs *fxfs_vfs_fs(struct super_block *sb)
{
	return sb->s_fs_info;
}

/* The inode `attr` describes, read in if it is not cached. */
static struct inode *fxfs_vfs_iget(struct super_block *sb, const struct fxfs_attr *attr)
{
	struct inode *inode = iget_locked(sb, attr->ino);

	if (!inode)
		return ERR_PTR(-ENOMEM);
	if (!(inode->i_state & I_NEW))
		return inode;
	inode->i_mode = attr->mode;
	set_nlink(inode, attr->nlink);
	i_size_write(inode, attr->size);
	simple_inode_init_ts(inode);
	if (S_ISDIR(attr->mode)) {
		inode->i_op = &fxfs_vfs_dir_inode_operations;
		inode->i_fop = &fxfs_vfs_dir_operations;
	} else {
		inode->i_fop = &fxfs_vfs_file_operations;
	}
	unlock_new_inode(inode);
	return inode;
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
	ssize_t total = 0;
	void *buf = kmalloc(PAGE_SIZE, GFP_KERNEL);

	if (!buf)
		return -ENOMEM;
	while (iov_iter_count(to)) {
		size_t chunk = min_t(size_t, iov_iter_count(to), PAGE_SIZE);
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
	kfree(buf);
	return total;
}

static const struct inode_operations fxfs_vfs_dir_inode_operations = {
	.lookup = fxfs_vfs_lookup,
};

static const struct file_operations fxfs_vfs_dir_operations = {
	.llseek = generic_file_llseek,
	.read = generic_read_dir,
	.iterate_shared = fxfs_vfs_iterate,
};

static const struct file_operations fxfs_vfs_file_operations = {
	.llseek = generic_file_llseek,
	.read_iter = fxfs_vfs_read_iter,
};

static const struct super_operations fxfs_vfs_super_operations = {
	.statfs = simple_statfs,
	.drop_inode = generic_delete_inode,
};

static int fxfs_vfs_fill_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_attr attr;
	struct inode *root;
	struct fxfs *fs = fxfs_nofs(fxfs_mount());
	int err;

	if (!fs)
		return -ENOMEM;
	sb->s_fs_info = fs;
	sb->s_magic = FXFS_MAGIC;
	sb->s_op = &fxfs_vfs_super_operations;
	sb->s_blocksize = PAGE_SIZE;
	sb->s_blocksize_bits = PAGE_SHIFT;
	sb->s_time_gran = 1;

	err = fxfs_nofs(fxfs_getattr(fs, FXFS_ROOT_INO, &attr));
	if (err)
		return err;
	root = fxfs_vfs_iget(sb, &attr);
	if (IS_ERR(root))
		return PTR_ERR(root);
	sb->s_root = d_make_root(root);
	return sb->s_root ? 0 : -ENOMEM;
}

static int fxfs_vfs_get_tree(struct fs_context *fc)
{
	return get_tree_nodev(fc, fxfs_vfs_fill_super);
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
	struct fxfs *fs = fxfs_vfs_fs(sb);
	unsigned int nofs;

	kill_anon_super(sb);
	if (fs) {
		nofs = memalloc_nofs_save();
		fxfs_unmount(fs);
		memalloc_nofs_restore(nofs);
	}
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
