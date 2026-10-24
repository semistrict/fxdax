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
	/* fxdax: /proc/fs/fxfs/<its device's tag>, what it waited on (fxfs_vfs_stats_show). */
	struct proc_dir_entry *stats;
	/* fxdax: readahead's reads, off their readers (fxfs_vfs_readahead); a follower has none. */
	struct workqueue_struct *readahead;
	/* fxdax: takes the size the host resizes the store to (fxfs_resize), off the interrupt. */
	struct work_struct resize;
	/*
	 * fxdax: a follower's (fxfs_follows, backlog doc-8): applies its writer's commits, as its
	 * device tells of them, off the interrupt (fxfs_follow).
	 */
	bool follows;
	struct work_struct follow;
	/*
	 * fxdax: the commits a follower's state is as of, as its readers may find it: every batch up
	 * to `follow_settled`'s commit settled, none past `follow_applied`'s applied
	 * (FXFS_IOC_FOLLOW_EPOCHS). Written by its batches alone.
	 */
	u64 follow_settled;
	u64 follow_applied;
	/*
	 * fxdax: a follower's fill barrier (TASK-42.10, doc-8): what readers hold shared as they
	 * read its trees and fill the kernel's caches from them (fxfs_vfs_fill_begin), and a batch
	 * exclusively, from before it drops what the kernel keeps of what it changes until it has
	 * applied the change and set what the kernel keeps anew: no reader finds, or caches, part of
	 * a batch, nor a cache stale after one it found. Taken after an inode's i_rwsem, which the
	 * VFS holds as it calls in (a directory's, to look up or read it) and a read(2) takes, and
	 * mmap_lock, which a fault holds, and before an inode's invalidate_lock; a batch takes no
	 * i_rwsem. How long batches waited for it, and how many did (FXFS_IOC_FOLLOW_STATS).
	 */
	struct rw_semaphore follow_barrier;
	u64 follow_batches;
	u64 follow_barrier_wait_ns;
	u64 follow_barrier_wait_max_ns;
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

/*
 * fxdax: FXFS_IOC_FOLLOW_STATS, on a follower's directory: its batches, and how long they waited
 * for its fill barrier, all told and at most, in nanoseconds.
 */
struct fxfs_follow_stats {
	__u64 batches;
	__u64 barrier_wait_ns;
	__u64 barrier_wait_max_ns;
};
#define FXFS_IOC_FOLLOW_STATS _IOR('x', 0x43, struct fxfs_follow_stats)

/*
 * fxdax: FXFS_IOC_CACHED_OBJECTS, on any directory: how many records Fxfs caches, which the
 * superblock's shrinker frees under memory pressure (fxfs_cached_objects).
 */
#define FXFS_IOC_CACHED_OBJECTS _IOR('x', 0x44, __u64)

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
 * must catch: a follower's batches dropping and settling nothing; its readers' fill barrier not
 * taken; its read(2)s' copies faulting, holding the barrier. All 0 but in the test.
 */
extern unsigned int fxfs_follow_apply_delay_us;
extern unsigned int fxfs_follow_iomap_delay_us;
extern bool fxfs_follow_skip_invalidate;
extern bool fxfs_follow_skip_barrier;
extern bool fxfs_follow_read_faults;

/* fxdax: whether `sb`'s filesystem follows its writer. */
static inline bool fxfs_vfs_follows(struct super_block *sb)
{
	return static_branch_unlikely(&fxfs_vfs_followers) && fxfs_vfs_info(sb)->follows;
}

/*
 * fxdax: a reader of a follower's `sb` holds its fill barrier shared from before it reads the
 * trees until what it read is in the kernel's caches, or copied out (vfs.h's fxfs_vfs_info); on a
 * writer, nothing.
 */
static inline void fxfs_vfs_fill_begin(struct super_block *sb)
{
	if (fxfs_vfs_follows(sb) && !fxfs_follow_skip_barrier)
		down_read(&fxfs_vfs_info(sb)->follow_barrier);
}

/* As fxfs_vfs_fill_begin, but only if no batch holds the barrier, or waits for it. */
static inline bool fxfs_vfs_fill_try(struct super_block *sb)
{
	return !fxfs_vfs_follows(sb) || fxfs_follow_skip_barrier ||
	       down_read_trylock(&fxfs_vfs_info(sb)->follow_barrier);
}

static inline void fxfs_vfs_fill_end(struct super_block *sb)
{
	if (fxfs_vfs_follows(sb) && !fxfs_follow_skip_barrier)
		up_read(&fxfs_vfs_info(sb)->follow_barrier);
}

/*
 * fxdax: a follower's read(2) of `iocb`'s file into `to` by `read`, a read_iter, its fill barrier
 * held a chunk at a time (super.c). A copy to the reader's memory that would fault, which may be a
 * mapping of a follower's file whose fault takes the barrier, stops short instead; the barrier let
 * go, the reader's pages are faulted in, and the read goes on.
 */
ssize_t fxfs_vfs_follower_read(struct kiocb *iocb, struct iov_iter *to,
			       ssize_t (*read)(struct kiocb *iocb, struct iov_iter *to));

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
/* fxdax: reflinks (FICLONE, FICLONERANGE, copy_file_range), for regular files, DAX or not. */
loff_t fxfs_vfs_remap_file_range(struct file *file_in, loff_t pos_in, struct file *file_out,
				 loff_t pos_out, loff_t len, unsigned int remap_flags);

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
	/* Its attribute flags (FS_*_FL), which Fxfs keeps in its mode (FXFS_MODE_*). */
	u32 fsflags;
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
/* fxdax: shows how the window is used, as lines of a name and a number (TASK-91.7). */
void fxfs_dax_show_stats(struct fxfs_dax *dax, struct seq_file *m);
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
 * fxdax: readies a DAX file's `len` bytes at `pos` for a reflink to or from them, which makes
 * another file's share their blocks, or them another's, the file growing to `size` bytes if it is
 * longer: with its inode's lock and invalidate_lock held for writing.
 */
int fxfs_dax_remap(struct inode *inode, loff_t pos, loff_t len, loff_t size);
/*
 * fxdax: a follower's writer changed file `ino`'s data from byte `start` to `end`, inclusive,
 * which the window's ranges of its chunks there may map as it was: drops them, to be mapped again
 * as needed. Through `inode`, the file's, if one is read in, whose invalidate_lock the caller
 * holds for writing: it breaks the inode's mappings of them first.
 */
int fxfs_dax_changed(struct fxfs_dax *dax, struct inode *inode, u64 ino, loff_t start,
		     loff_t end);
/*
 * fxdax: a follower's DAX file, shrunk by its writer to `size`, its new size set: the guest's
 * mappings of its pages past it go, as a truncate's do. With its inode's invalidate_lock held for
 * writing.
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
static inline void fxfs_dax_show_stats(struct fxfs_dax *dax, struct seq_file *m) {}
static inline void fxfs_dax_inode_init(struct fxfs_dax_inode *di) {}
static inline void fxfs_dax_set_inode(struct inode *inode) {}
static inline void fxfs_dax_evict(struct inode *inode) {}
static inline int fxfs_dax_resize_begin(struct inode *inode, loff_t size)
{
	return 0;
}
static inline void fxfs_dax_resize_end(struct inode *inode, loff_t size) {}
static inline int fxfs_dax_remap(struct inode *inode, loff_t pos, loff_t len, loff_t size)
{
	return 0;
}
#endif

#endif
