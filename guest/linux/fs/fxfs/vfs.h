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

/* A regular file's DAX state (dax.c). */
struct fxfs_dax_inode {
	/* Guards `ranges`, and which pages of them are mapped. */
	struct rw_semaphore sem;
	/* The window's ranges mapping chunks of the file, by chunk. */
	struct xarray ranges;
	/* On the filesystem's list of inodes with writable window pages, if it has some. */
	struct list_head writable;
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
/* Frees `dax` once the filesystem is unmounted. */
void fxfs_dax_destroy(struct fxfs_dax *dax);
void fxfs_dax_inode_init(struct fxfs_dax_inode *di);
/* Makes the regular file `inode` a DAX inode. */
void fxfs_dax_set_inode(struct inode *inode);
/* Forgets the window's mappings of `inode`, which the kernel evicts. */
void fxfs_dax_evict(struct inode *inode);
/*
 * Before and after a DAX file's size changes to `size`, under its inode's lock: the first waits
 * for the pages past the size to be unused, and zeroes the last page past it; the second unmaps
 * what is past it from the window.
 */
int fxfs_dax_resize_begin(struct inode *inode, loff_t size);
void fxfs_dax_resize_end(struct inode *inode, loff_t size);
#else
static inline struct fxfs_dax *fxfs_dax_create(struct super_block *sb, struct fxfs *fs)
{
	return NULL;
}
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
