/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The extent-store device the filesystem lives on (drivers/virtio_extent, backlog doc-4), as the
 * filesystem's Rust uses it: its requests, with plain arguments, completed through a callback.
 * Rust declares the same in guest/rust/fxfs-linux's device module.
 */
#ifndef _FS_FXFS_DEVICE_H
#define _FS_FXFS_DEVICE_H

struct seq_file;

#include <linux/types.h>

/* A bound extent-store device. */
struct fxfs_device;

/* A WRITE may carry VIRTIO_EXTENT_WRITE_COMMIT. */
#define FXFS_DEVICE_WRITE_COMMIT (1 << 0)
/* The device takes FREEZE, and INFO says the branch (VIRTIO_EXTENT_F_FREEZE). */
#define FXFS_DEVICE_FREEZE (1 << 1)
/*
 * fxdax: the device follows a store another VM writes, read-only (VIRTIO_EXTENT_F_READ_ONLY):
 * ADVANCE, LET_GO, PIN and UNPIN work, and nothing that writes.
 */
#define FXFS_DEVICE_READ_ONLY (1 << 2)
/* fxdax: a FREE may carry VIRTIO_EXTENT_FREE_MANY, more extents in its data. */
#define FXFS_DEVICE_FREE_MANY (1 << 3)

/* What the filesystem needs to know of a device. */
struct fxfs_device_info {
	u32 block_size;
	/* The most bytes one READ or WRITE moves, from anywhere in memory: a multiple of blocks. */
	u32 max_io;
	/* The most bytes the root holds. */
	u32 max_root;
	/* FXFS_DEVICE_*. */
	u32 flags;
	/* The branch whose extents the device's filesystem writes. */
	u64 branch;
	/* The most bytes one extent holds. */
	u64 max_extent;
};

/*
 * The device tagged with the `len` bytes of `tag`, filling in `info`, or NULL if there is none.
 * The caller puts it once done with it (fxfs_device_put).
 */
struct fxfs_device *fxfs_device_find(const char *tag, size_t len, struct fxfs_device_info *info);
void fxfs_device_put(struct fxfs_device *dev);

/*
 * A request: its type (VIRTIO_EXTENT_T_*) and flags, the extent it names (READ, WRITE, FREE, PIN,
 * UNPIN), and where in it (READ, WRITE), or LET_GO's epoch (offset), and its data: READ's buffer,
 * WRITE's data, COMMIT's root, ROOT's buffer (max_root bytes, and a struct virtio_extent_root_len
 * after them), INFO's struct virtio_extent_info, or ADVANCE's struct virtio_extent_epoch. The data
 * must be kmalloc'd or vmalloc'd, and stay until the request is done. A WRITE with
 * VIRTIO_EXTENT_WRITE_ROOT has a root too, which its commit sets, of root_len bytes: kmalloc'd.
 */
struct fxfs_device_op {
	u32 type;
	u32 flags;
	u64 branch;
	u64 sequence;
	u64 offset;
	void *data;
	u32 len;
	u32 root_len;
	void *root;
};

/* Called once the device completes a request, from its interrupt: it must not sleep. */
typedef void (*fxfs_device_done_t)(void *arg, int status);

/*
 * Submits `op`, and calls `done(arg, status)` once the device completes it, with 0 or the device's
 * -errno. Returns 0, or a -errno if it couldn't submit it, when it doesn't call `done`. May sleep.
 */
int fxfs_device_submit(struct fxfs_device *dev, const struct fxfs_device_op *op,
		       fxfs_device_done_t done, void *arg);

/*
 * Requests that seal the extents open (doc-4), a COMMIT, or a WRITE with a barrier, are submitted
 * between fxfs_device_seal_begin and fxfs_device_seal_end, once the device completes it, which
 * count seals (fxfs_device_seals): extents Fxfs takes for sealed from a seal's beginning are on the
 * device only at its end, and the DAX window maps sealed extents alone (doc-13). `sealed(arg)` is
 * called once each seal ends; it may not wait for the filesystem's Rust, which seals.
 */
typedef void (*fxfs_device_sealed_t)(void *arg);
void fxfs_device_set_sealed(struct fxfs_device *dev, fxfs_device_sealed_t sealed, void *arg);

/*
 * The store's capacity, and the shrink the guest is asked to take, or 0, as the device's
 * configuration says now: 0, or -EOPNOTSUPP if it says neither (VIRTIO_EXTENT_F_RESIZE).
 */
int fxfs_device_capacity(struct fxfs_device *dev, u64 *capacity, u64 *asked);
/*
 * Has `changed(arg)` called as the device's configuration changes: the host resizes the store, or
 * a shrink is asked; or, a follower's (FXFS_DEVICE_READ_ONLY), its writer commits. Or nothing, if `changed` is NULL. Called from the device's interrupt: it
 * must not sleep.
 */
typedef void (*fxfs_device_changed_t)(void *arg);
void fxfs_device_set_changed(struct fxfs_device *dev, fxfs_device_changed_t changed, void *arg);
/* fxdax: a follower's device's writer's latest commit, as its configuration says now. */
u64 fxfs_device_committed(struct fxfs_device *dev);
/* Waits for a seal in progress, if any, to end. */
void fxfs_device_wait_unsealed(struct fxfs_device *dev);
/*
 * The seal count: how many seals have begun and ended, odd while one is in progress, whose request
 * may seal extents opened meanwhile.
 */
u64 fxfs_device_seals(struct fxfs_device *dev);
void fxfs_device_seal_begin(struct fxfs_device *dev);
void fxfs_device_seal_end(struct fxfs_device *dev);

struct dax_device;

/*
 * The DAX window's dax_device, if files' data can be mapped from it, at the page size, which is a
 * block: fxdax: the window copies on write (doc-13), mapping sealed extents and zeros alone,
 * read-only, a store to a page making it the VM's own, private, until a MAP or UNMAP over it; or,
 * a follower's, takes no store. Sets `window_len`.
 */
struct dax_device *fxfs_device_dax(struct fxfs_device *dev, u64 *window_len);
/*
 * fxdax: the most private pages the device holds at once, past which a store stops the VM; and
 * whether it follows a store another VM writes, read-only (FXFS_DEVICE_READ_ONLY): its window
 * takes no store, nor maps zeros.
 */
u64 fxfs_device_private_max(struct fxfs_device *dev);
bool fxfs_device_read_only(struct fxfs_device *dev);
/*
 * Maps `len` bytes at `offset` in the extent {branch, sequence} at `window_offset` in the window,
 * read-only (MAP), or unmaps `len` bytes there (UNMAP), and waits for the device. fxdax: `ahead`
 * if the guest accesses the pages next, as a read does, which the device then maps for it ahead of
 * its accesses, if it can (MAP_AHEAD, TASK-112.1).
 */
int fxfs_device_map(struct fxfs_device *dev, u64 branch, u64 sequence, u64 offset, u64 len,
		    u64 window_offset, bool ahead);

/*
 * fxdax: one of fxfs_device_map_many's maps, as fxfs_device_map's arguments say it; or, with
 * `zeros`, fxfs_device_map_zeros's, its extent ignored.
 */
struct fxfs_device_map {
	u64 branch;
	u64 sequence;
	u64 offset;
	u64 len;
	u64 window_offset;
	bool ahead;
	bool zeros;
};

/*
 * fxdax: the `count` maps `maps`, in as few requests as the device takes (MAP_MANY), or one each:
 * a chunk's pieces (TASK-91.4). The first error fails them, whichever others the device mapped.
 */
int fxfs_device_map_many(struct fxfs_device *dev, const struct fxfs_device_map *maps,
			 size_t count);
int fxfs_device_unmap(struct fxfs_device *dev, u64 window_offset, u64 len);
/*
 * fxdax: maps zeros at `len` bytes of the window at `window_offset` (MAP_ZEROS), read-
 * only, as holes and pages past a file's end are; and makes `len` bytes there private now, as
 * stores would, mapped for the guest ahead of its accesses (PRIVATE): -ENOMEM past private_max.
 * Each waits for the device.
 */
int fxfs_device_map_zeros(struct fxfs_device *dev, u64 window_offset, u64 len);
int fxfs_device_private(struct fxfs_device *dev, u64 window_offset, u64 len);
/* fxdax: shows what the window's requests took, as lines of a name and a number (TASK-91.7). */
void fxfs_device_show_stats(struct fxfs_device *dev, struct seq_file *m);

#endif
