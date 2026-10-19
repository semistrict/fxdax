/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * What Fxfs's VFS glue (super.c) and its DAX (dax.c) share: the superblock's and inodes' state,
 * and calls into Rust.
 */
#ifndef _FS_FXFS_VFS_H
#define _FS_FXFS_VFS_H

#include <linux/fs.h>
#include <linux/jump_label.h>
#include <linux/list.h>
#include <linux/mutex.h>
#include <linux/rwsem.h>
#include <linux/sched/mm.h>
#include <linux/seqlock.h>
#include <linux/srcu.h>
#include <linux/workqueue.h>
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
	/* fxdax: takes the size the host resizes the store to (fxfs_resize), off the interrupt. */
	struct work_struct resize;
	/*
	 * fxdax: a follower's (fxfs_follows, backlog doc-8): applies its writer's commits, as its
	 * device tells of them, off the interrupt (fxfs_follow); and the primitives its batches and
	 * readers keep (fxfs.h's fxfs_vfs_follow_*). The seqcount is the kernel's seqcount_t, written
	 * holding the follower's mutex with raw_write_seqcount_begin, not a seqcount_mutex_t, whose
	 * write_seqcount_begin disables preemption: applying a batch sleeps. Its readers spin a few
	 * tries at most, then take the mutex.
	 */
	bool follows;
	struct work_struct follow;
	struct mutex follow_lock;
	seqcount_t follow_seq;
	struct srcu_struct follow_srcu;
	bool follow_srcu_ready;
	/*
	 * fxdax: the commits a follower's state is as of, as its readers may find it: every batch up
	 * to `follow_settled`'s commit invalidated, none past `follow_applied`'s applied
	 * (FXFS_IOC_FOLLOW_EPOCHS). Written by its batches alone.
	 */
	u64 follow_settled;
	u64 follow_applied;
};

/*
 * fxdax: FXFS_IOC_FOLLOW_EPOCHS, on a follower's directory: the commits its state is as of. A read
 * between two calls was of a state as of a commit from the first's `settled` to the second's
 * `applied` (the followers' history checker, TASK-42.7).
 */
struct fxfs_follow_epochs {
	__u64 settled;
	__u64 applied;
};
#define FXFS_IOC_FOLLOW_EPOCHS _IOR('x', 0x42, struct fxfs_follow_epochs)

static inline struct fxfs_vfs_info *fxfs_vfs_info(struct super_block *sb)
{
	return sb->s_fs_info;
}

static inline struct fxfs *fxfs_vfs_fs(struct super_block *sb)
{
	return fxfs_vfs_info(sb)->fs;
}

/*
 * fxdax: whether any filesystem follows its writer: a static key, so that a writer's mounts, with
 * no follower mounted, take no branch for the follower's paths.
 */
DECLARE_STATIC_KEY_FALSE(fxfs_vfs_followers);

/*
 * fxdax: the followers' torture test's knobs (TASK-42.7, doc-8 testing 6), module parameters
 * (fxfs.NAME=VALUE on the kernel's command line): delays, in microseconds, that widen the windows
 * the protocol closes; and mutations, each switching off the rule that closes one, which the test
 * must catch: a follower's invalidation after a batch skipped; its DAX faults' invalidate_lock,
 * which excludes a batch's invalidation, not taken. All 0 but in the test.
 */
extern unsigned int fxfs_follow_apply_delay_us;
extern unsigned int fxfs_follow_iomap_delay_us;
extern bool fxfs_follow_skip_invalidate;
extern bool fxfs_follow_skip_fault_lock;

/* fxdax: whether `sb`'s filesystem follows its writer. */
static inline bool fxfs_vfs_follows(struct super_block *sb)
{
	return static_branch_unlikely(&fxfs_vfs_followers) && fxfs_vfs_info(sb)->follows;
}

/*
 * fxdax: a regular file's open and release, which hold it on a follower (fxfs_hold), for DAX or
 * not (super.c).
 */
int fxfs_vfs_file_open(struct inode *inode, struct file *file);
int fxfs_vfs_file_release(struct inode *inode, struct file *file);

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
	/*
	 * fxdax: a follower's directory's generation: how many batches changed its entries, which
	 * its dentries were looked up at (d_time) and revalidate against.
	 */
	atomic64_t gen;
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
/*
 * Waits for `dax`'s background work, once nothing writes, and stops it, before the filesystem
 * unmounts, or freezes, leaving nothing open on the device; fxfs_dax_thaw starts it again.
 */
void fxfs_dax_quiesce(struct fxfs_dax *dax);
void fxfs_dax_thaw(struct fxfs_dax *dax);
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
/*
 * fxdax: a follower's writer changed file `ino`'s data from byte `start` to `end`, inclusive,
 * which the window's ranges of its chunks there may map as it was: drops them, to be mapped again
 * as needed. Through `inode`, the file's, if one is read in, whose lock and invalidate_lock the
 * caller holds for writing: it breaks the inode's mappings of them first.
 */
int fxfs_dax_changed(struct fxfs_dax *dax, struct inode *inode, u64 ino, loff_t start,
		     loff_t end);
/*
 * fxdax: a follower's DAX file, shrunk by its writer to `size`, its new size set: the guest's
 * mappings of its pages past it go, as a truncate's do. With its inode's lock and invalidate_lock
 * held for writing.
 */
int fxfs_dax_shrunk(struct inode *inode, loff_t size);
#else
static inline int fxfs_dax_changed(struct fxfs_dax *dax, struct inode *inode, u64 ino,
				   loff_t start, loff_t end)
{
	return 0;
}
static inline int fxfs_dax_shrunk(struct inode *inode, loff_t size)
{
	return 0;
}
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
static inline void fxfs_dax_thaw(struct fxfs_dax *dax) {}
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
