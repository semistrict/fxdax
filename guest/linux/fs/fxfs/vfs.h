/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/*
 * What Fxfs's VFS glue (super.c) and its DAX (dax.c) share: the superblock's and inodes' state,
 * and calls into Rust.
 */
#ifndef _FS_FXFS_VFS_H
#define _FS_FXFS_VFS_H

#include <linux/fs.h>
#include <linux/list.h>
#include <linux/rwsem.h>
#include <linux/sched/mm.h>
#include <linux/xarray.h>

#include "fxfs.h"

/*
 * Evaluates `call`, a call into Rust, in a NOFS allocation scope: Rust's allocator may reclaim
 * memory, which must not recurse into filesystems.
 */
#define fxfs_nofs(call)                                        \
	({                                                     \
		unsigned int __nofs = memalloc_nofs_save();    \
		typeof(call) __ret = (call);                   \
		memalloc_nofs_restore(__nofs);                 \
		__ret;                                         \
	})

struct virtio_extent;
struct fxfs_dax;

/* A superblock's: the device it is on, the filesystem, once mounted, and its DAX, if it has one. */
struct fxfs_vfs_info {
	struct virtio_extent *vext;
	struct fxfs *fs;
	struct fxfs_dax *dax;
};

static inline struct fxfs_vfs_info *fxfs_vfs_info(struct super_block *sb)
{
	return sb->s_fs_info;
}

static inline struct fxfs *fxfs_vfs_fs(struct super_block *sb)
{
	return fxfs_vfs_info(sb)->fs;
}

/* fallocate(2), for regular files, DAX or not (super.c). */
long fxfs_vfs_fallocate(struct file *file, int mode, loff_t offset, loff_t len);
/* llseek(2), with SEEK_DATA and SEEK_HOLE, for regular files, DAX or not (super.c). */
loff_t fxfs_vfs_llseek(struct file *file, loff_t offset, int whence);
/* fsync(2), for files of any kind, DAX or not (super.c). */
int fxfs_vfs_fsync(struct file *file, loff_t start, loff_t end, int datasync);

struct fxfs_dax_file;

/* A regular file's DAX state (dax.c). */
struct fxfs_dax_inode {
	/* The file's ranges of the window, which outlive the inode, if it is a DAX inode. */
	struct fxfs_dax_file *file;
};

struct fxfs_vfs_inode {
	struct fxfs_dax_inode dax;
	struct inode vfs;
};

static inline struct fxfs_vfs_inode *fxfs_vfs_inode(struct inode *inode)
{
	return container_of(inode, struct fxfs_vfs_inode, vfs);
}

/* dax.c: files' data mapped from the extent-store device's DAX window. */
#if IS_ENABLED(CONFIG_FS_DAX)

/*
 * The DAX of the filesystem `fs`, mounted on `sb`, if its device's window can map files' data;
 * NULL if it can't, or an ERR_PTR.
 */
struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs);
/* Waits for `dax`'s regions growing in the background, before a sync. */
void fxfs_dax_settle(struct fxfs_dax *dax);
/* Waits for `dax`'s background work, once nothing writes, before the filesystem unmounts. */
void fxfs_dax_quiesce(struct fxfs_dax *dax);
/* Frees `dax` once the filesystem is unmounted. */
void fxfs_dax_destroy(struct fxfs_dax *dax);
void fxfs_dax_inode_init(struct fxfs_dax_inode *di);
/* Makes the regular file `inode` a DAX inode. */
void fxfs_dax_set_inode(struct inode *inode);
/*
 * Detaches `inode`, which the kernel evicts, from its file's window mappings, which stay for the
 * next inode read in for it: unless no name links it, when they go.
 */
void fxfs_dax_evict(struct inode *inode);
/*
 * Before and after a DAX file's size changes to `size`, under its inode's lock: the first waits
 * for the pages past the size to be unused, and zeroes the last page past it; the second unmaps
 * what is past it from the window.
 */
int fxfs_dax_resize_begin(struct inode *inode, loff_t size);
/*
 * Before and after a hole is punched in a DAX file from `start` to `end`, under its inode's lock:
 * the first zeroes the range's partial pages through the window, and drops the window's ranges of
 * its whole pages, holding the invalidate_lock, which the second releases.
 */
int fxfs_dax_punch_begin(struct inode *inode, loff_t start, loff_t end);
void fxfs_dax_punch_end(struct inode *inode);
void fxfs_dax_resize_end(struct inode *inode, loff_t size);
#else
static inline struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs)
{
	return NULL;
}
static inline void fxfs_dax_settle(struct fxfs_dax *dax) {}
static inline int fxfs_dax_punch_begin(struct inode *inode, loff_t start, loff_t end)
{
	return -EOPNOTSUPP;
}
static inline void fxfs_dax_punch_end(struct inode *inode) {}
static inline void fxfs_dax_quiesce(struct fxfs_dax *dax) {}
static inline void fxfs_dax_destroy(struct fxfs_dax *dax) {}
static inline void fxfs_dax_inode_init(struct fxfs_dax_inode *di) {}
static inline void fxfs_dax_set_inode(struct inode *inode) {}
static inline void fxfs_dax_evict(struct inode *inode) {}
static inline int fxfs_dax_resize_begin(struct inode *inode, loff_t size)
{
	return 0;
}
static inline void fxfs_dax_resize_end(struct inode *inode, loff_t size) {}
#endif

#endif
