// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * Fxfs's VFS glue: the kernel's superblock, inode, directory and file operations, over the
 * filesystem in Rust (fxfs.h).
 *
 * Rust's allocator may reclaim memory, which must not recurse into filesystems, so every call into
 * Rust runs in a NOFS allocation scope.
 *
 * Files' data is in the device's DAX window, if it has one (dax.c), as the host shares it: one copy
 * of it in memory. Otherwise it is in the page cache, which writeback writes to Fxfs, contiguous
 * dirty folios in one write, and reads fill, readahead's folios in one read. The kernel keeps
 * inodes' times, which write_inode writes back.
 */

#include <linux/backing-dev.h>
#include <linux/dax.h>
#include <linux/delay.h>
#include <linux/exportfs.h>
#include <linux/falloc.h>
#include <linux/fiemap.h>
#include <linux/fs.h>
#include <linux/fs_context.h>
#include <linux/init.h>
#include <linux/minmax.h>
#include <linux/module.h>
#include <linux/namei.h>
#include <linux/pagemap.h>
#include <linux/posix_acl.h>
#include <linux/posix_acl_xattr.h>
#include <linux/sched/mm.h>
#include <linux/seq_file.h>
#include <linux/slab.h>
#include <linux/statfs.h>
#include <linux/uio.h>
#include <linux/virtio_extent.h>
#include <linux/workqueue.h>
#include <linux/writeback.h>
#include <linux/xattr.h>

#include "device.h"
#include "fxfs.h"
#include "rt.h"
#include "vfs.h"

#define FXFS_MAGIC 0x46784673 /* "FxFs" */

/* The most bytes a read or write moves to or from Rust at once. */
#define FXFS_IO_MAX (1 << 20)


static const struct inode_operations fxfs_vfs_dir_inode_operations;
static const struct inode_operations fxfs_vfs_file_inode_operations;
static const struct inode_operations fxfs_vfs_symlink_inode_operations;
static const struct inode_operations fxfs_vfs_special_inode_operations;
static const struct file_operations fxfs_vfs_dir_operations;
static const struct file_operations fxfs_vfs_file_operations;
static const struct address_space_operations fxfs_vfs_aops;
static const struct dentry_operations fxfs_vfs_follower_dentry_operations;

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
		/* Small files too: they share extents, and the window's mappings of them. */
		if (fxfs_vfs_info(sb)->dax)
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
 * Writes what the kernel keeps of `inode` to Fxfs: its mode, owner and times, and with
 * FXFS_SET_SIZE in `valid`, its size; and the changes to its extended attributes.
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

	return fxfs_nofs(fxfs_write_inode(fxfs_vfs_fs(inode->i_sb), inode->i_ino, &set));
}

static struct dentry *fxfs_vfs_lookup(struct inode *dir, struct dentry *dentry,
				      unsigned int flags)
{
	/* fxdax: a follower's dentry is as of its directory's generation before the lookup. */
	unsigned long gen = atomic64_read(&fxfs_vfs_inode(dir)->gen);
	struct fxfs_attr attr;
	struct inode *inode = NULL;
	struct dentry *found;
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
	WRITE_ONCE(dentry->d_time, gen);
	found = d_splice_alias(inode, dentry);
	if (!IS_ERR_OR_NULL(found))
		WRITE_ONCE(found->d_time, gen);
	return found;
}

/*
 * fxdax: a follower's dentry is valid while its directory's entries are as they were when it was
 * looked up: a batch that changed them bumps the directory's generation (fxfs_vfs_changed), and
 * the dentry is looked up again, as FUSE's and NFS's are. Its name, or its absence, may be gone.
 */
static int fxfs_vfs_d_revalidate(struct inode *dir, const struct qstr *name,
				 struct dentry *dentry, unsigned int flags)
{
	return READ_ONCE(dentry->d_time) == (unsigned long)atomic64_read(&fxfs_vfs_inode(dir)->gen);
}

static const struct dentry_operations fxfs_vfs_follower_dentry_operations = {
	.d_revalidate = fxfs_vfs_d_revalidate,
};

/*
 * Instantiates `dentry`, a new name in `dir`, with the inode `attr` describes, and `dir` as
 * `dir_attr` describes it: or, without `dir_attr`, as Fxfs left it, but for its times, which are
 * the kernel's, written back with it (write_inode).
 */
static int fxfs_vfs_instantiate(struct inode *dir, struct dentry *dentry,
				const struct fxfs_attr *attr, const struct fxfs_attr *dir_attr)
{
	struct inode *inode = fxfs_vfs_iget(dir->i_sb, attr);

	if (dir_attr) {
		fxfs_vfs_update_attr(dir, dir_attr);
	} else {
		inode_set_mtime_to_ts(dir, inode_set_ctime_current(dir));
		mark_inode_dirty(dir);
	}
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

/* The extended attribute an ACL of `type` is stored as. */
static const char *fxfs_vfs_acl_name(int type)
{
	switch (type) {
	case ACL_TYPE_ACCESS:
		return XATTR_NAME_POSIX_ACL_ACCESS;
	case ACL_TYPE_DEFAULT:
		return XATTR_NAME_POSIX_ACL_DEFAULT;
	default:
		return NULL;
	}
}

/* `inode`'s POSIX ACL of `type`, stored as an extended attribute, as btrfs stores it. */
static struct posix_acl *fxfs_vfs_get_acl(struct inode *inode, int type, bool rcu)
{
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	const char *name = fxfs_vfs_acl_name(type);
	struct posix_acl *acl;
	ssize_t size;
	void *value;

	if (rcu)
		return ERR_PTR(-ECHILD);
	if (!name)
		return ERR_PTR(-EINVAL);
	size = fxfs_nofs(fxfs_getxattr(fs, inode->i_ino, name, strlen(name), NULL, 0));
	if (size == -ENODATA || size == 0)
		return NULL;
	if (size < 0)
		return ERR_PTR(size);
	value = kmalloc(size, GFP_KERNEL);
	if (!value)
		return ERR_PTR(-ENOMEM);
	size = fxfs_nofs(fxfs_getxattr(fs, inode->i_ino, name, strlen(name), value, size));
	if (size > 0)
		acl = posix_acl_from_xattr(&init_user_ns, value, size);
	else
		acl = size == -ENODATA || size == 0 ? NULL : ERR_PTR(size);
	kfree(value);
	return acl;
}

/*
 * Stores `acl`, or removes it if NULL, as `inode`'s ACL of `type`: written back with the inode,
 * which it marks dirty, as its other extended attributes are.
 */
static int fxfs_vfs_store_acl(struct inode *inode, struct posix_acl *acl, int type)
{
	struct fxfs *fs = fxfs_vfs_fs(inode->i_sb);
	const char *name = fxfs_vfs_acl_name(type);
	size_t size = 0;
	void *value;
	int err;

	if (!name)
		return -EINVAL;
	if (type == ACL_TYPE_DEFAULT && !S_ISDIR(inode->i_mode))
		return acl ? -EACCES : 0;
	if (acl) {
		size = posix_acl_xattr_size(acl->a_count);
		value = kmalloc(size, GFP_KERNEL);
		if (!value)
			return -ENOMEM;
		err = posix_acl_to_xattr(&init_user_ns, acl, value, size);
		if (err >= 0)
			err = fxfs_nofs(fxfs_setxattr(fs, inode->i_ino, name, strlen(name), value,
						      size, 0));
		kfree(value);
	} else {
		err = fxfs_nofs(fxfs_removexattr(fs, inode->i_ino, name, strlen(name)));
		if (err == -ENODATA)
			err = 0;
	}
	if (err)
		return err;
	set_cached_acl(inode, type, acl);
	mark_inode_dirty(inode);
	return 0;
}

static int fxfs_vfs_set_acl(struct mnt_idmap *idmap, struct dentry *dentry, struct posix_acl *acl,
			    int type)
{
	struct inode *inode = d_inode(dentry);
	umode_t old_mode = inode->i_mode;
	int err;

	if (type == ACL_TYPE_ACCESS && acl) {
		err = posix_acl_update_mode(idmap, inode, &inode->i_mode, &acl);
		if (err)
			return err;
	}
	err = fxfs_vfs_store_acl(inode, acl, type);
	if (err) {
		inode->i_mode = old_mode;
		return err;
	}
	inode_set_ctime_current(inode);
	return 0;
}

/*
 * The ACLs a new inode in `dir` inherits, if its default ACL says, and its mode, which they, or
 * the umask without them, restrict (posix_acl_create): with SB_POSIXACL, the VFS leaves the umask
 * to the filesystem.
 */
static int fxfs_vfs_new_acls(struct inode *dir, umode_t *mode, struct posix_acl **default_acl,
			     struct posix_acl **acl)
{
	if (S_ISLNK(*mode)) {
		*default_acl = NULL;
		*acl = NULL;
		return 0;
	}
	return posix_acl_create(dir, mode, default_acl, acl);
}

/* Stores the ACLs a new inode inherited, and releases them. */
static int fxfs_vfs_init_acls(struct inode *inode, struct posix_acl *default_acl,
			      struct posix_acl *acl)
{
	int err = 0;

	if (default_acl) {
		err = fxfs_vfs_store_acl(inode, default_acl, ACL_TYPE_DEFAULT);
		posix_acl_release(default_acl);
	}
	if (acl) {
		if (!err)
			err = fxfs_vfs_store_acl(inode, acl, ACL_TYPE_ACCESS);
		posix_acl_release(acl);
	}
	return err;
}

/* Creates `dentry` in `dir` as `mode` says, owned by the caller. */
static int fxfs_vfs_mknod(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			  umode_t mode, dev_t rdev)
{
	struct posix_acl *default_acl, *acl;
	struct fxfs_attr attr, dir_attr;
	u32 gid;
	int err;

	err = fxfs_vfs_new_acls(dir, &mode, &default_acl, &acl);
	if (err)
		return err;
	gid = fxfs_vfs_new_gid(dir, &mode);
	err = fxfs_nofs(fxfs_create(fxfs_vfs_fs(dir->i_sb), dir->i_ino, dentry->d_name.name,
				    dentry->d_name.len, mode, new_encode_dev(rdev),
				    from_kuid(&init_user_ns, current_fsuid()), gid, &attr,
				    &dir_attr));
	/* A directory's times Fxfs sets as it creates a directory, which counts in its links. */
	if (!err)
		err = fxfs_vfs_instantiate(dir, dentry, &attr, S_ISDIR(mode) ? &dir_attr : NULL);
	if (err) {
		posix_acl_release(default_acl);
		posix_acl_release(acl);
		return err;
	}
	/*
	 * No extended attribute a write would remove (security.capability): its first write needn't
	 * look for one (file_remove_privs). Setting one clears S_NOSEC.
	 */
	inode_has_no_xattr(d_inode(dentry));
	return fxfs_vfs_init_acls(d_inode(dentry), default_acl, acl);
}

/* Creates an unnamed regular file in `dir`, which a link may name later (O_TMPFILE). */
static int fxfs_vfs_tmpfile(struct mnt_idmap *idmap, struct inode *dir, struct file *file,
			    umode_t mode)
{
	struct posix_acl *default_acl, *acl;
	struct fxfs_attr attr;
	struct inode *inode;
	u32 gid;
	int err;

	err = fxfs_vfs_new_acls(dir, &mode, &default_acl, &acl);
	if (err)
		return err;
	gid = fxfs_vfs_new_gid(dir, &mode);
	err = fxfs_nofs(fxfs_tmpfile(fxfs_vfs_fs(dir->i_sb), dir->i_ino, mode,
				     from_kuid(&init_user_ns, current_fsuid()), gid, &attr));
	if (err) {
		posix_acl_release(default_acl);
		posix_acl_release(acl);
		return err;
	}
	inode = fxfs_vfs_iget(dir->i_sb, &attr);
	if (IS_ERR(inode)) {
		posix_acl_release(default_acl);
		posix_acl_release(acl);
		return PTR_ERR(inode);
	}
	/* Its one reference, the graveyard's, which the kernel doesn't count as a link. */
	d_tmpfile(file, inode);
	/* As fxfs_vfs_mknod's. */
	inode_has_no_xattr(inode);
	err = fxfs_vfs_init_acls(inode, default_acl, acl);
	if (err)
		return err;
	return finish_open_simple(file, 0);
}

static int fxfs_vfs_create(struct mnt_idmap *idmap, struct inode *dir, struct dentry *dentry,
			   umode_t mode, bool excl)
{
	return fxfs_vfs_mknod(idmap, dir, dentry, mode | S_IFREG, 0);
}

/* The directory, in `dentry`: NULL, the dentry made, or the error. */
static struct dentry *fxfs_vfs_mkdir(struct mnt_idmap *idmap, struct inode *dir,
				     struct dentry *dentry, umode_t mode)
{
	return ERR_PTR(fxfs_vfs_mknod(idmap, dir, dentry, mode | S_IFDIR, 0));
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
	/* An O_TMPFILE file, linked for the first time, leaves the graveyard. */
	bool unnamed = inode->i_nlink == 0;
	int err = fxfs_nofs(fxfs_link(fxfs_vfs_fs(dir->i_sb), inode->i_ino, dir->i_ino,
				      dentry->d_name.name, dentry->d_name.len, unnamed, &attr,
				      &dir_attr));

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

/*
 * Removes `dentry`'s name from `dir`. A file's, symlink's or special file's goes later, in a batch
 * with others (fxfs_unlink_later): the kernel holds `dir`'s lock meanwhile, and has checked what
 * could fail. A directory's goes now: only Fxfs knows whether it is empty.
 */
static int fxfs_vfs_unlink(struct inode *dir, struct dentry *dentry)
{
	struct inode *inode = d_inode(dentry);
	struct fxfs *fs = fxfs_vfs_fs(dir->i_sb);
	struct fxfs_attr dir_attr;
	int replaced;

	if (!S_ISDIR(inode->i_mode)) {
		int err = fxfs_nofs(fxfs_unlink_later(fs, dir->i_ino, dentry->d_name.name,
						      dentry->d_name.len, inode->i_ino));

		if (err)
			return err;
		fxfs_vfs_removed(inode, FXFS_REPLACED_OBJECT);
		/*
		 * Written back with the directory (write_inode), or a file's fsync in it: the batch
		 * leaves them be. Unlike ext4's, they aren't committed with the name: after a crash the
		 * directory's times may be older than the change.
		 */
		inode_set_mtime_to_ts(dir, inode_set_ctime_current(dir));
		mark_inode_dirty(dir);
		return 0;
	}
	replaced = fxfs_nofs(fxfs_unlink(fs, dir->i_ino, dentry->d_name.name, dentry->d_name.len,
					 inode->i_ino, &dir_attr));
	if (replaced < 0)
		return replaced;
	fxfs_vfs_removed(inode, replaced);
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
/*
 * Makes `inode` `size` bytes long, with the attributes `iattr` sets, if any, and writes them back.
 * Its inode lock held.
 */
static int fxfs_vfs_resize(struct inode *inode, loff_t size, struct mnt_idmap *idmap,
			   struct iattr *iattr)
{
	int err;

	if (IS_DAX(inode)) {
		err = fxfs_dax_resize_begin(inode, size);
		if (err)
			return err;
	}
	/* The page cache past the size goes first, so writeback can't write it back. */
	truncate_setsize(inode, size);
	if (iattr)
		setattr_copy(idmap, inode, iattr);
	err = fxfs_vfs_write_attr(inode, FXFS_SET_SIZE);
	if (IS_DAX(inode))
		fxfs_dax_resize_end(inode, size);
	return err;
}

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
	return fxfs_vfs_resize(inode, size, idmap, iattr);
}

/* The most of a file's data whose pieces one call asks Fxfs for, and so the most pieces. */
#define FXFS_EXTENT_WINDOW (1 << 20)
#define FXFS_EXTENT_BATCH (FXFS_EXTENT_WINDOW / PAGE_SIZE)

/*
 * Calls `fn` with each piece of `inode`'s data within `start` to `end`, in order, a window at a
 * time, until it returns nonzero, which this returns; or 0, or a -errno.
 */
static int fxfs_vfs_each_extent(struct inode *inode, loff_t start, loff_t end,
				int (*fn)(void *arg, const struct fxfs_dax_extent *e), void *arg)
{
	struct fxfs_dax_extent *extents;
	loff_t at = round_down(start, PAGE_SIZE);
	int ret = 0;

	extents = kmalloc_array(FXFS_EXTENT_BATCH, sizeof(*extents), GFP_KERNEL);
	if (!extents)
		return -ENOMEM;
	while (!ret && at < end) {
		u64 len = min_t(u64, FXFS_EXTENT_WINDOW, end - at);
		ssize_t count, i;

		count = fxfs_nofs(fxfs_dax_extents(fxfs_vfs_fs(inode->i_sb), inode->i_ino, at, len,
						   extents, FXFS_EXTENT_BATCH));
		if (count < 0) {
			ret = count;
			break;
		}
		for (i = 0; i < count && !ret; i++)
			ret = fn(arg, &extents[i]);
		at += len;
	}
	kfree(extents);
	return ret;
}

/* SEEK_DATA's or SEEK_HOLE's search from `offset`, which finds `found`. */
struct fxfs_vfs_seek {
	loff_t offset;
	int whence;
	loff_t found;
};

static int fxfs_vfs_seek_extent(void *arg, const struct fxfs_dax_extent *e)
{
	struct fxfs_vfs_seek *seek = arg;
	loff_t start = e->offset, end = e->offset + e->len;

	if (end <= seek->offset)
		return 0;
	if (seek->whence == SEEK_DATA) {
		seek->found = max(start, seek->offset);
		return 1;
	}
	/* Data from `found` on, so far: a gap before this piece is the hole. */
	if (start > seek->found)
		return 1;
	seek->found = max(seek->found, end);
	return 0;
}

/* llseek(2), with SEEK_DATA and SEEK_HOLE from Fxfs's pieces of the file's data. */
loff_t fxfs_vfs_llseek(struct file *file, loff_t offset, int whence)
{
	struct inode *inode = file->f_mapping->host;
	struct fxfs_vfs_seek seek = { .offset = offset, .whence = whence };
	loff_t size;
	int err;

	if (whence != SEEK_DATA && whence != SEEK_HOLE)
		return generic_file_llseek(file, offset, whence);
	inode_lock_shared(inode);
	size = i_size_read(inode);
	if (offset < 0 || offset >= size) {
		inode_unlock_shared(inode);
		return -ENXIO;
	}
	seek.found = whence == SEEK_DATA ? size : offset;
	/* Data in the page cache has no pieces until written back. */
	err = filemap_write_and_wait(inode->i_mapping);
	if (!err)
		err = fxfs_vfs_each_extent(inode, offset, size, fxfs_vfs_seek_extent, &seek);
	inode_unlock_shared(inode);
	if (err < 0)
		return err;
	if (whence == SEEK_DATA && !err)
		return -ENXIO;
	return vfs_setpos(file, min(seek.found, size), inode->i_sb->s_maxbytes);
}

/* fiemap's extent being gathered: pieces of one Fxfs extent, contiguous in it and in the file. */
struct fxfs_vfs_fiemap {
	struct fiemap_extent_info *fieinfo;
	bool have;
	u64 logical, len;
	u64 branch, sequence, extent_end;
};

/* Reports the extent gathered, if any, with `flags`: 1 once the caller's array is full. */
static int fxfs_vfs_fiemap_flush(struct fxfs_vfs_fiemap *m, u32 flags)
{
	if (!m->have)
		return 0;
	m->have = false;
	/* An extent store's extents have no address on a device. */
	return fiemap_fill_next_extent(m->fieinfo, m->logical, 0, m->len,
				       FIEMAP_EXTENT_UNKNOWN | flags);
}

static int fxfs_vfs_fiemap_extent(void *arg, const struct fxfs_dax_extent *e)
{
	struct fxfs_vfs_fiemap *m = arg;
	int ret;

	if (m->have && m->logical + m->len == e->offset && m->branch == e->branch &&
	    m->sequence == e->sequence && m->extent_end == e->extent_offset) {
		m->len += e->len;
		m->extent_end += e->len;
		return 0;
	}
	ret = fxfs_vfs_fiemap_flush(m, 0);
	if (ret)
		return ret;
	m->have = true;
	m->logical = e->offset;
	m->len = e->len;
	m->branch = e->branch;
	m->sequence = e->sequence;
	m->extent_end = e->extent_offset + e->len;
	return 0;
}

/* FS_IOC_FIEMAP: the file's data's extents, without addresses (FIEMAP_EXTENT_UNKNOWN). */
static int fxfs_vfs_fiemap(struct inode *inode, struct fiemap_extent_info *fieinfo, u64 start,
			   u64 len)
{
	struct fxfs_vfs_fiemap m = { .fieinfo = fieinfo };
	loff_t size, end;
	int err = fiemap_prep(inode, fieinfo, start, &len, 0);

	if (err)
		return err;
	inode_lock_shared(inode);
	size = i_size_read(inode);
	end = start >= size ? start : min_t(u64, size, start + len);
	err = filemap_write_and_wait(inode->i_mapping);
	if (!err)
		err = fxfs_vfs_each_extent(inode, start, end, fxfs_vfs_fiemap_extent, &m);
	if (!err)
		err = fxfs_vfs_fiemap_flush(&m, end == size ? FIEMAP_EXTENT_LAST : 0);
	inode_unlock_shared(inode);
	return err < 0 ? err : 0;
}

/* Writes zeros over the `len` bytes of `inode` at `pos`, within a page, to Fxfs. */
static int fxfs_vfs_write_zeros(struct inode *inode, loff_t pos, loff_t len)
{
	void *zeros = kzalloc(len, GFP_KERNEL);
	int err;

	if (!zeros)
		return -ENOMEM;
	err = fxfs_nofs(fxfs_write(fxfs_vfs_fs(inode->i_sb), inode->i_ino, pos, zeros, len));
	kfree(zeros);
	return err;
}

/*
 * Makes `start` to `end` of `inode`, within its size, a hole: its partial pages zeroed, its whole
 * pages' blocks freed. With the inode's lock held.
 */
static int fxfs_vfs_punch(struct inode *inode, loff_t start, loff_t end)
{
	loff_t first = round_up(start, PAGE_SIZE), last = round_down(end, PAGE_SIZE);
	int err;

	if (end <= start)
		return 0;
	if (IS_DAX(inode)) {
		err = fxfs_dax_punch_begin(inode, start, end);
		if (err)
			return err;
	} else {
		filemap_invalidate_lock(inode->i_mapping);
		err = filemap_write_and_wait_range(inode->i_mapping, start, end - 1);
		if (!err)
			truncate_pagecache_range(inode, start, end - 1);
		if (!err && first > last) {
			err = fxfs_vfs_write_zeros(inode, start, end - start);
		} else {
			if (!err && start < first)
				err = fxfs_vfs_write_zeros(inode, start, first - start);
			if (!err && last < end)
				err = fxfs_vfs_write_zeros(inode, last, end - last);
		}
	}
	if (!err && first < last)
		err = fxfs_nofs(fxfs_punch(fxfs_vfs_fs(inode->i_sb), inode->i_ino, first,
					   last - first));
	if (IS_DAX(inode))
		fxfs_dax_punch_end(inode);
	else
		filemap_invalidate_unlock(inode->i_mapping);
	return err;
}

/*
 * fallocate(2): makes the file at least `offset` + `len` bytes long, unless FALLOC_FL_KEEP_SIZE;
 * with FALLOC_FL_PUNCH_HOLE or FALLOC_FL_ZERO_RANGE, makes the range within the file a hole
 * first. Blocks never written read as zeros, and take no space until written: an extent store
 * can't reserve them, so a write to them can still find no space. Other modes aren't supported.
 */
long fxfs_vfs_fallocate(struct file *file, int mode, loff_t offset, loff_t len)
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
	if (!err && (mode & (FALLOC_FL_PUNCH_HOLE | FALLOC_FL_ZERO_RANGE))) {
		err = file_modified(file);
		if (!err)
			err = fxfs_vfs_punch(inode, offset, min(end, i_size_read(inode)));
	}
	if (!err && !(mode & FALLOC_FL_KEEP_SIZE) && end > i_size_read(inode)) {
		err = file_modified(file);
		if (!err)
			err = fxfs_vfs_resize(inode, end, NULL, NULL);
	}
	inode_unlock(inode);
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

/* The most bytes of entries an open directory reads from Fxfs at once (fxfs_vfs_dir_cursor). */
#define FXFS_DIR_BUFFER (32 << 10)

/* An entry read from Fxfs, in a cursor's buffer: its name follows, padded to 8 bytes. */
struct fxfs_vfs_dir_entry {
	u64 ino;
	u32 mode;
	u16 len;
	char name[];
};

/*
 * Where reading an open directory got to: the position after the last entry it read, and that
 * entry's name, which the next read continues after, whatever was removed meanwhile. And the
 * entries read from Fxfs ahead of it, from `buffered_pos` on: each of the directory's reads, from
 * Fxfs, seeks its trees' layers for where it left off.
 */
struct fxfs_vfs_dir_cursor {
	loff_t pos;
	size_t len;
	char name[NAME_MAX + 1];
	char *buffer;
	size_t buffered, at;
	loff_t buffered_pos;
	/* Whether the buffer holds the directory's last entries. */
	bool last;
};

static size_t fxfs_vfs_dir_entry_size(size_t len)
{
	return ALIGN(sizeof(struct fxfs_vfs_dir_entry) + len, 8);
}

/* Adds an entry Fxfs read to the cursor's buffer, if it has room: false if not. */
static bool fxfs_vfs_buffer_entry(void *arg, const char *name, size_t len, u64 ino, u32 mode)
{
	struct fxfs_vfs_dir_cursor *cursor = arg;
	struct fxfs_vfs_dir_entry *entry;

	if (len > NAME_MAX || cursor->buffered + fxfs_vfs_dir_entry_size(len) > FXFS_DIR_BUFFER)
		return false;
	entry = (struct fxfs_vfs_dir_entry *)(cursor->buffer + cursor->buffered);
	entry->ino = ino;
	entry->mode = mode;
	entry->len = len;
	memcpy(entry->name, name, len);
	cursor->buffered += fxfs_vfs_dir_entry_size(len);
	return true;
}

/*
 * Reads the directory's entries from position `pos` on into the cursor's buffer, as many as fit:
 * after the cursor's name if it is the entry before `pos`, as a read continuing does.
 */
static int fxfs_vfs_dir_fill(struct inode *inode, struct fxfs_vfs_dir_cursor *cursor, loff_t pos)
{
	bool resume = cursor->len && cursor->pos == pos;
	int err;

	cursor->buffered = 0;
	cursor->at = 0;
	cursor->buffered_pos = pos;
	err = fxfs_nofs(fxfs_readdir(fxfs_vfs_fs(inode->i_sb), inode->i_ino, pos - 2,
				     resume ? cursor->name : NULL, cursor->len,
				     fxfs_vfs_buffer_entry, cursor));
	/* Whatever didn't fit is still to read. */
	cursor->last = !err && cursor->buffered + fxfs_vfs_dir_entry_size(NAME_MAX) <=
			       FXFS_DIR_BUFFER;
	if (err)
		cursor->buffered = 0;
	return err;
}

static int fxfs_vfs_dir_open(struct inode *inode, struct file *file)
{
	file->private_data = kzalloc(sizeof(struct fxfs_vfs_dir_cursor), GFP_KERNEL);
	return file->private_data ? 0 : -ENOMEM;
}

static int fxfs_vfs_dir_release(struct inode *inode, struct file *file)
{
	struct fxfs_vfs_dir_cursor *cursor = file->private_data;

	kvfree(cursor->buffer);
	kfree(cursor);
	return 0;
}

/*
 * Positions 0 and 1 are "." and "..", and the filesystem's entries follow, in order. A read
 * continuing where the last left off continues after the last name it read, which removing
 * entries before it doesn't move; another, after a seek, counts entries. Entries come from the
 * cursor's buffer, which a read filled ahead, as long as the read continues where it left off.
 */
static int fxfs_vfs_iterate(struct file *file, struct dir_context *ctx)
{
	struct inode *inode = file_inode(file);
	struct fxfs_vfs_dir_cursor *cursor = file->private_data;
	struct fxfs_vfs_dir_entry *entry;
	int err;

	if (!dir_emit_dots(file, ctx))
		return 0;
	if (!cursor->buffer) {
		cursor->buffer = kvmalloc(FXFS_DIR_BUFFER, GFP_KERNEL);
		if (!cursor->buffer)
			return -ENOMEM;
	}
	for (;;) {
		if (cursor->buffered_pos != ctx->pos || cursor->at == cursor->buffered) {
			/* At the end of the buffer, which held the last entries. */
			if (cursor->buffered_pos == ctx->pos && cursor->last && cursor->buffered)
				return 0;
			err = fxfs_vfs_dir_fill(inode, cursor, ctx->pos);
			if (err)
				return err;
			if (!cursor->buffered)
				return 0;
		}
		entry = (struct fxfs_vfs_dir_entry *)(cursor->buffer + cursor->at);
		if (!dir_emit(ctx, entry->name, entry->len, entry->ino,
			      fs_umode_to_dtype(entry->mode)))
			return 0;
		ctx->pos++;
		memcpy(cursor->name, entry->name, entry->len);
		cursor->len = entry->len;
		cursor->pos = ctx->pos;
		cursor->at += fxfs_vfs_dir_entry_size(entry->len);
		cursor->buffered_pos = ctx->pos;
	}
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
static int fxfs_vfs_write_begin(const struct kiocb *iocb, struct address_space *mapping,
				loff_t pos, unsigned int len, struct folio **foliop, void **fsdata)
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
static int fxfs_vfs_write_end(const struct kiocb *iocb, struct address_space *mapping,
			      loff_t pos, unsigned int len, unsigned int copied,
			      struct folio *folio, void *fsdata)
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

/*
 * Writes back the file's data and times, and the filesystem's changes, in order (fxfs_fsync),
 * sealing what was stored through the DAX window. And a file's directory's times, which creates
 * and unlinks leave to the kernel (write_inode), as a journaling filesystem's fsync of a file
 * commits the change to its directory that made it: so that a sync of the directory after it has
 * nothing of its own to commit.
 */
int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file_inode(file);
	int err = file_write_and_wait_range(file, start, end);

	if (!err)
		err = sync_inode_metadata(inode, 1);
	if (!err && !S_ISDIR(inode->i_mode)) {
		struct dentry *parent = dget_parent(file_dentry(file));

		err = sync_inode_metadata(d_inode(parent), 1);
		dput(parent);
	}
	if (!err)
		err = fxfs_nofs(fxfs_fsync(fxfs_vfs_fs(inode->i_sb)));
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
	.tmpfile = fxfs_vfs_tmpfile,
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
	.get_inode_acl = fxfs_vfs_get_acl,
	.set_acl = fxfs_vfs_set_acl,
};

static const struct inode_operations fxfs_vfs_file_inode_operations = {
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
	.fiemap = fxfs_vfs_fiemap,
	.get_inode_acl = fxfs_vfs_get_acl,
	.set_acl = fxfs_vfs_set_acl,
};

static const struct inode_operations fxfs_vfs_symlink_inode_operations = {
	.get_link = fxfs_vfs_get_link,
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
};

static const struct inode_operations fxfs_vfs_special_inode_operations = {
	.setattr = fxfs_vfs_setattr,
	.listxattr = fxfs_vfs_listxattr,
	.get_inode_acl = fxfs_vfs_get_acl,
	.set_acl = fxfs_vfs_set_acl,
};

static const struct file_operations fxfs_vfs_dir_operations = {
	.open = fxfs_vfs_dir_open,
	.release = fxfs_vfs_dir_release,
	.llseek = generic_file_llseek,
	.read = generic_read_dir,
	.iterate_shared = fxfs_vfs_iterate,
	.fsync = fxfs_vfs_fsync,
};

int fxfs_vfs_file_open(struct inode *inode, struct file *file)
{
	int err = generic_file_open(inode, file);
	unsigned int nofs;

	if (!err && fxfs_vfs_follows(inode->i_sb)) {
		nofs = memalloc_nofs_save();
		fxfs_hold(fxfs_vfs_fs(inode->i_sb), inode->i_ino);
		memalloc_nofs_restore(nofs);
	}
	return err;
}

int fxfs_vfs_file_release(struct inode *inode, struct file *file)
{
	unsigned int nofs;

	if (fxfs_vfs_follows(inode->i_sb)) {
		nofs = memalloc_nofs_save();
		fxfs_release(fxfs_vfs_fs(inode->i_sb), inode->i_ino);
		memalloc_nofs_restore(nofs);
	}
	return 0;
}

static const struct file_operations fxfs_vfs_file_operations = {
	.open = fxfs_vfs_file_open,
	.release = fxfs_vfs_file_release,
	.llseek = fxfs_vfs_llseek,
	.read_iter = generic_file_read_iter,
	.write_iter = generic_file_write_iter,
	.mmap = generic_file_mmap,
	.splice_read = filemap_splice_read,
	.splice_write = iter_file_splice_write,
	.fsync = fxfs_vfs_fsync,
	.fallocate = fxfs_vfs_fallocate,
};

/*
 * Closes the inode's handles in Fxfs, and frees an inode no name links; a directory is gone
 * already.
 */
static void fxfs_vfs_evict_inode(struct inode *inode)
{
	unsigned int nofs;

	/* No DAX entries are left for truncate_inode_pages_final, which has none to find. */
	dax_break_layout_final(inode);
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
	/* Frozen, it was made durable as it froze, and nothing has changed since. */
	if (sb->s_writers.frozen == SB_FREEZE_COMPLETE)
		return 0;
	/* What the writes so far map, mapped before the sync seals it. */
	if (fxfs_vfs_info(sb)->dax)
		fxfs_dax_settle(fxfs_vfs_info(sb)->dax);
	return fxfs_nofs(fxfs_sync(fxfs_vfs_fs(sb)));
}

/*
 * Freezing (fsfreeze, FIFREEZE): the VFS stops writers and syncs, and the filesystem is made
 * durable, and holds its background work, compacting and freeing, which writes nothing until it
 * thaws: nothing is open on the device, the point a snapshot of the VM is taken at (fxdax's
 * TASK-39.1). Thawing, it learns the device's branch, which a VM restored from the snapshot has
 * its own of.
 */
static int fxfs_vfs_freeze_fs(struct super_block *sb)
{
	struct fxfs_dax *dax = fxfs_vfs_info(sb)->dax;
	int err;

	if (dax)
		fxfs_dax_quiesce(dax);
	err = fxfs_nofs(fxfs_freeze(fxfs_vfs_fs(sb)));
	if (err && dax)
		fxfs_dax_thaw(dax);
	return err;
}

static int fxfs_vfs_unfreeze_fs(struct super_block *sb)
{
	int err = fxfs_nofs(fxfs_thaw(fxfs_vfs_fs(sb)));

	if (fxfs_vfs_info(sb)->dax)
		fxfs_dax_thaw(fxfs_vfs_info(sb)->dax);
	return err;
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
	atomic64_set(&fi->gen, 0);
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

/*
 * What the mount is, as /proc/mounts shows it: Fxfs takes no options of its own, but files map
 * their data from the device's DAX window, if it has one (dax), and the filesystem follows another
 * VM's writes, if its device is a follower's (follow).
 */
static int fxfs_vfs_show_options(struct seq_file *m, struct dentry *root)
{
	struct fxfs_vfs_info *info = fxfs_vfs_info(root->d_sb);

	if (info->dax)
		seq_puts(m, ",dax");
	if (info->follows)
		seq_puts(m, ",follow");
	return 0;
}

static const struct super_operations fxfs_vfs_super_operations = {
	.show_options = fxfs_vfs_show_options,
	.alloc_inode = fxfs_vfs_alloc_inode,
	.free_inode = fxfs_vfs_free_inode,
	.statfs = fxfs_vfs_statfs,
	.evict_inode = fxfs_vfs_evict_inode,
	.write_inode = fxfs_vfs_write_inode,
	.sync_fs = fxfs_vfs_sync_fs,
	.freeze_fs = fxfs_vfs_freeze_fs,
	.unfreeze_fs = fxfs_vfs_unfreeze_fs,
};

/*
 * File handles (name_to_handle_at, open_by_handle_at, NFS): an inode's 64-bit number, generation 0,
 * as Fxfs never reuses object IDs. Without parent pointers in Fxfs, a directory's handle can't be
 * reconnected to its path.
 */
static int fxfs_vfs_encode_fh(struct inode *inode, u32 *fh, int *max_len, struct inode *parent)
{
	struct fid *fid = (struct fid *)fh;

	if (parent)
		return FILEID_INVALID;
	if (*max_len < 3) {
		*max_len = 3;
		return FILEID_INVALID;
	}
	fid->i64.ino = inode->i_ino;
	fid->i64.gen = 0;
	*max_len = 3;
	return FILEID_INO64_GEN;
}

static struct dentry *fxfs_vfs_fh_to_dentry(struct super_block *sb, struct fid *fid, int fh_len,
					    int fh_type)
{
	struct fxfs_attr attr;
	struct inode *inode;
	int err;

	if (fh_type != FILEID_INO64_GEN || fh_len < 3)
		return NULL;
	inode = ilookup(sb, fid->i64.ino);
	if (!inode) {
		err = fxfs_nofs(fxfs_getattr(fxfs_vfs_fs(sb), fid->i64.ino, &attr));
		if (err)
			return ERR_PTR(err == -ENOENT ? -ESTALE : err);
		inode = fxfs_vfs_iget(sb, &attr);
	}
	return d_obtain_alias(inode);
}

static const struct export_operations fxfs_vfs_export_operations = {
	.encode_fh = fxfs_vfs_encode_fh,
	.fh_to_dentry = fxfs_vfs_fh_to_dentry,
};

/* fxdax: takes the size the host resized the store to. */
static void fxfs_vfs_resize_work(struct work_struct *work)
{
	struct fxfs_vfs_info *info = container_of(work, struct fxfs_vfs_info, resize);
	int err = fxfs_nofs(fxfs_resize(info->fs));

	if (err)
		pr_warn("fxfs: resizing: %d\n", err);
}

/* fxdax: the host resized the store, or asks the guest to shrink: from the device's interrupt. */
static void fxfs_vfs_resized(void *arg)
{
	struct fxfs_vfs_info *info = arg;

	schedule_work(&info->resize);
}

DEFINE_STATIC_KEY_FALSE(fxfs_vfs_followers);

/* fxdax: the torture test's knobs (vfs.h). */
unsigned int fxfs_follow_apply_delay_us;
module_param_named(follow_apply_delay_us, fxfs_follow_apply_delay_us, uint, 0644);
MODULE_PARM_DESC(follow_apply_delay_us,
		 "A follower's delay between applying a batch and invalidating it (tests)");
unsigned int fxfs_follow_iomap_delay_us;
module_param_named(follow_iomap_delay_us, fxfs_follow_iomap_delay_us, uint, 0644);
MODULE_PARM_DESC(follow_iomap_delay_us,
		 "A follower's delay between an iomap's extents and its mapping (tests)");
bool fxfs_follow_skip_invalidate;
module_param_named(follow_skip_invalidate, fxfs_follow_skip_invalidate, bool, 0644);
MODULE_PARM_DESC(follow_skip_invalidate,
		 "A follower invalidates nothing after a batch: a mutation the tests catch");

/* fxdax: a follower's batch, as its device tells of its writer's commits. */
static void fxfs_vfs_follow_work(struct work_struct *work)
{
	struct fxfs_vfs_info *info = container_of(work, struct fxfs_vfs_info, follow);
	int err = fxfs_nofs(fxfs_follow(info->fs));

	if (err)
		pr_warn_ratelimited("fxfs: following the writer: %d\n", err);
}

/* fxdax: a follower's device tells of its writer's commits: from its interrupt. */
static void fxfs_vfs_committed(void *arg)
{
	struct fxfs_vfs_info *info = arg;

	schedule_work(&info->follow);
}

/*
 * fxdax: a follower's primitives (fxfs.h), of the superblock `vfs`. The seqcount is written with
 * raw_write_seqcount_begin holding the follower's mutex: applying a batch sleeps, which
 * seqcount_mutex_t's write_seqcount_begin, disabling preemption, forbids (vfs.h).
 */
static struct fxfs_vfs_info *fxfs_vfs_follow_info(void *vfs)
{
	return fxfs_vfs_info(vfs);
}

u32 fxfs_vfs_follow_sequence(void *vfs)
{
	return raw_read_seqcount(&fxfs_vfs_follow_info(vfs)->follow_seq);
}

bool fxfs_vfs_follow_moved(void *vfs, u32 start)
{
	return read_seqcount_retry(&fxfs_vfs_follow_info(vfs)->follow_seq, start);
}

void fxfs_vfs_follow_write_begin(void *vfs)
{
	struct fxfs_vfs_info *info = fxfs_vfs_follow_info(vfs);

	mutex_lock(&info->follow_lock);
	raw_write_seqcount_begin(&info->follow_seq);
}

void fxfs_vfs_follow_write_end(void *vfs)
{
	struct fxfs_vfs_info *info = fxfs_vfs_follow_info(vfs);

	raw_write_seqcount_end(&info->follow_seq);
	mutex_unlock(&info->follow_lock);
	/* The window between a batch applied and its invalidation, widened in tests. */
	if (fxfs_follow_apply_delay_us)
		fsleep(fxfs_follow_apply_delay_us);
}

void fxfs_vfs_follow_lock(void *vfs)
{
	mutex_lock(&fxfs_vfs_follow_info(vfs)->follow_lock);
}

void fxfs_vfs_follow_unlock(void *vfs)
{
	mutex_unlock(&fxfs_vfs_follow_info(vfs)->follow_lock);
}

int fxfs_vfs_follow_grace_enter(void *vfs)
{
	return srcu_read_lock(&fxfs_vfs_follow_info(vfs)->follow_srcu);
}

void fxfs_vfs_follow_grace_leave(void *vfs, int index)
{
	srcu_read_unlock(&fxfs_vfs_follow_info(vfs)->follow_srcu, index);
}

void fxfs_vfs_follow_grace_wait(void *vfs)
{
	synchronize_srcu(&fxfs_vfs_follow_info(vfs)->follow_srcu);
}

/*
 * fxdax: a follower's file `ino`, `inode` if one is read in, whose data changed from `start` to
 * `end`, exclusive: what the window maps of it, or the page cache holds, goes. With an inode,
 * its lock and its invalidate_lock held for writing, which a read(2) and a fault, and readahead,
 * hold shared as they read and map: one that read the file as it was maps it before this.
 */
static void fxfs_vfs_changed_data(struct super_block *sb, struct inode *inode, u64 ino,
				  u64 start, u64 end)
{
	struct fxfs_dax *dax = fxfs_vfs_info(sb)->dax;
	loff_t first = min_t(u64, start, LLONG_MAX);
	loff_t last = end > LLONG_MAX ? LLONG_MAX : (loff_t)end - 1;
	int err;

	if (last < first)
		return;
	if (dax) {
		err = fxfs_dax_changed(dax, inode, ino, first, last);
		if (err)
			pr_warn("fxfs: dropping inode %llu's window mappings: %d\n", ino, err);
		return;
	}
	if (!inode || !S_ISREG(inode->i_mode))
		return;
	err = invalidate_inode_pages2_range(inode->i_mapping, first >> PAGE_SHIFT,
					    last >> PAGE_SHIFT);
	if (err)
		pr_warn("fxfs: invalidating inode %llu's pages: %d\n", ino, err);
}

/*
 * fxdax: a follower's inode takes the attributes `attr` a batch left its object with, under its
 * lock; or, gone, no name links it. A file grown has its window's ranges past its old end dropped
 * first, which were as large as the file was (fxfs_dax_fit), holding its invalidate_lock until it
 * is as long as it grew: a fault meanwhile would map the range again for the old size.
 */
static void fxfs_vfs_changed_attr(struct super_block *sb, struct inode *inode, u64 ino,
				  const struct fxfs_attr *attr)
{
	if (!inode) {
		/* Its window's ranges were as large as it was. */
		if (attr && S_ISREG(attr->mode))
			fxfs_vfs_changed_data(sb, NULL, ino, 0, U64_MAX);
		return;
	}
	inode_lock(inode);
	if (!attr) {
		clear_nlink(inode);
	} else if ((attr->mode & S_IFMT) != (inode->i_mode & S_IFMT)) {
		pr_warn("fxfs: inode %llu changed type\n", ino);
		clear_nlink(inode);
	} else {
		bool grows = S_ISREG(inode->i_mode) && attr->size > i_size_read(inode);

		if (grows) {
			filemap_invalidate_lock(inode->i_mapping);
			fxfs_vfs_changed_data(sb, inode, ino, i_size_read(inode), U64_MAX);
		}
		fxfs_vfs_set_attr(inode, attr);
		if (grows)
			filemap_invalidate_unlock(inode->i_mapping);
	}
	inode_unlock(inode);
}

void fxfs_vfs_changed(void *vfs, u32 what, u64 ino, u64 start, u64 end,
		      const struct fxfs_attr *attr)
{
	struct super_block *sb = vfs;
	struct inode *inode;

	/* Only a follower's batches change what the kernel keeps under it. */
	if (WARN_ON_ONCE(!fxfs_vfs_follows(sb)) || fxfs_follow_skip_invalidate)
		return;
	inode = ilookup(sb, ino);
	switch (what) {
	case FXFS_CHANGED_DATA:
	case FXFS_CHANGED_DELETED:
		if (inode) {
			inode_lock(inode);
			filemap_invalidate_lock(inode->i_mapping);
		}
		fxfs_vfs_changed_data(sb, inode, ino, start, end);
		if (inode) {
			filemap_invalidate_unlock(inode->i_mapping);
			inode_unlock(inode);
		}
		break;
	case FXFS_CHANGED_ATTR:
		fxfs_vfs_changed_attr(sb, inode, ino, attr);
		break;
	case FXFS_CHANGED_ENTRIES:
		if (inode)
			atomic64_inc(&fxfs_vfs_inode(inode)->gen);
		break;
	}
	iput(inode);
}

/* Mounts the filesystem on the extent-store device the mount's source names by its tag. */
static int fxfs_vfs_fill_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_vfs_info *info = fxfs_vfs_info(sb);
	struct fxfs_attr attr;
	struct inode *root;
	struct fxfs *fs;
	int err;

	/* Writeback for the page cache: the default backing device has none. */
	err = super_setup_bdi(sb);
	if (err)
		return err;
	/* fxdax: what a follower's batches and readers keep, readied before it mounts. */
	mutex_init(&info->follow_lock);
	seqcount_init(&info->follow_seq);
	err = init_srcu_struct(&info->follow_srcu);
	if (err)
		return err;
	info->follow_srcu_ready = true;
	INIT_WORK(&info->follow, fxfs_vfs_follow_work);
	err = fxfs_nofs(fxfs_mount(fc->source, strlen(fc->source),
				   (u64)totalram_pages() << PAGE_SHIFT,
				   fc->sb_flags & SB_RDONLY, sb, &fs, &attr));
	if (err)
		return err;
	fxfs_vfs_info(sb)->fs = fs;
	INIT_WORK(&fxfs_vfs_info(sb)->resize, fxfs_vfs_resize_work);
	/* fxdax: read-only, it writes nothing; a follower's device tells of its writer's commits. */
	if (fxfs_read_only(fs))
		sb->s_flags |= SB_RDONLY;
	info->follows = fxfs_follows(fs);
	if (info->follows) {
		static_branch_inc(&fxfs_vfs_followers);
		set_default_d_op(sb, &fxfs_vfs_follower_dentry_operations);
		fxfs_device_set_resized(fxfs_dax_device(fs), fxfs_vfs_committed, info);
		/* Those it made since the device's view opened. */
		schedule_work(&info->follow);
	} else {
		fxfs_device_set_resized(fxfs_dax_device(fs), fxfs_vfs_resized, info);
	}
	fxfs_vfs_info(sb)->dax = fxfs_dax_create(sb, fs);
	if (IS_ERR(fxfs_vfs_info(sb)->dax)) {
		err = PTR_ERR(fxfs_vfs_info(sb)->dax);
		fxfs_vfs_info(sb)->dax = NULL;
		return err;
	}
	sb->s_magic = FXFS_MAGIC;
	sb->s_op = &fxfs_vfs_super_operations;
	sb->s_export_op = &fxfs_vfs_export_operations;
	sb->s_xattr = fxfs_vfs_xattr_handlers;
	sb->s_blocksize = PAGE_SIZE;
	sb->s_blocksize_bits = PAGE_SHIFT;
	sb->s_maxbytes = MAX_LFS_FILESIZE;
	sb->s_time_gran = 1;
	/*
	 * A write looks at an inode's security attributes once, not each time, until they change,
	 * as block devices' filesystems get from get_tree_bdev(): each look is a lookup in Fxfs.
	 */
	sb->s_flags |= SB_NOSEC;
	/* ACLs, as extended attributes (fxfs_vfs_get_acl). */
	sb->s_flags |= SB_POSIXACL;

	root = fxfs_vfs_iget(sb, &attr);
	if (IS_ERR(root))
		return PTR_ERR(root);
	sb->s_root = d_make_root(root);
	return sb->s_root ? 0 : -ENOMEM;
}

/* Whether `sb` is on the device `fc` mounts, which the info `fc` holds for a new one names. */
static int fxfs_vfs_test_super(struct super_block *sb, struct fs_context *fc)
{
	struct fxfs_vfs_info *info = fc->s_fs_info;

	return fxfs_vfs_info(sb)->vext == info->vext;
}

/*
 * Readies a new superblock, which sget_fc gives the info `fc` holds. It runs under sb_lock, a
 * spinlock: what it needs is allocated before (fxfs_vfs_get_tree).
 */
static int fxfs_vfs_set_super(struct super_block *sb, struct fs_context *fc)
{
	return set_anon_super_fc(sb, fc);
}

/*
 * One superblock per device, as get_tree_keyed has it: mounting a device mounted already shares
 * its filesystem.
 */
static int fxfs_vfs_get_tree(struct fs_context *fc)
{
	struct virtio_extent *vext;
	struct fxfs_vfs_info *info;
	struct super_block *sb;
	int err;

	if (!fc->source)
		return invalf(fc, "fxfs: the source must be an extent store's tag");
	vext = virtio_extent_find(fc->source);
	if (!vext)
		return -ENOENT;
	info = kzalloc(sizeof(*info), GFP_KERNEL);
	if (!info)
		return -ENOMEM;
	info->vext = vext;
	fc->s_fs_info = info;
	sb = sget_fc(fc, fxfs_vfs_test_super, fxfs_vfs_set_super);
	/* A new superblock took it; a shared one, or none, did not. */
	kfree(fc->s_fs_info);
	fc->s_fs_info = NULL;
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

/* fxdax: a filesystem mounted read-only, or following its writer, stays so. */
static int fxfs_vfs_reconfigure(struct fs_context *fc)
{
	struct super_block *sb = fc->root->d_sb;

	if (!(fc->sb_flags & SB_RDONLY) && fxfs_read_only(fxfs_vfs_fs(sb)))
		return invalf(fc, "fxfs: mounted read-only, or following a writer: it stays so");
	return 0;
}

static const struct fs_context_operations fxfs_vfs_context_operations = {
	.get_tree = fxfs_vfs_get_tree,
	.reconfigure = fxfs_vfs_reconfigure,
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

	if (info->fs) {
		/*
		 * No resize, nor a writer's commit, comes once unmounting: a follower's batch, which
		 * invalidates inodes, ends before they go.
		 */
		fxfs_device_set_resized(fxfs_dax_device(info->fs), NULL, NULL);
		cancel_work_sync(&info->resize);
		cancel_work_sync(&info->follow);
	}
	kill_anon_super(sb);
	if (info->dax)
		fxfs_dax_quiesce(info->dax);
	if (info->fs) {
		nofs = memalloc_nofs_save();
		fxfs_unmount(info->fs);
		memalloc_nofs_restore(nofs);
	}
	/* Once unmounted: the last seals call the DAX's sealer. */
	if (info->dax)
		fxfs_dax_destroy(info->dax);
	if (info->follow_srcu_ready)
		cleanup_srcu_struct(&info->follow_srcu);
	if (info->follows)
		static_branch_dec(&fxfs_vfs_followers);
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
	err = fxfs_rt_init();
	if (err) {
		kmem_cache_destroy(fxfs_vfs_inode_cache);
		return err;
	}
	fxfs_init();
	err = register_filesystem(&fxfs_vfs_type);
	if (err) {
		fxfs_rt_exit();
		kmem_cache_destroy(fxfs_vfs_inode_cache);
	}
	return err;
}

static void __exit fxfs_vfs_exit(void)
{
	unregister_filesystem(&fxfs_vfs_type);
	fxfs_rt_exit();
	rcu_barrier();
	kmem_cache_destroy(fxfs_vfs_inode_cache);
}

module_init(fxfs_vfs_init);
module_exit(fxfs_vfs_exit);

MODULE_DESCRIPTION("Fxfs, in Rust");
MODULE_LICENSE("Dual BSD/GPL");
MODULE_ALIAS_FS("fxfs");
