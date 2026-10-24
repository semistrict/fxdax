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
 * must be kmalloc'd or vmalloc'd, and stay until the request is done.
 */
struct fxfs_device_op {
	u32 type;
	u32 flags;
	u64 branch;
	u64 sequence;
	u64 offset;
	void *data;
	u32 len;
	u32 reserved;
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
 * Stores through the DAX window's writable mappings, and requests that seal the extents they map,
 * which make them read-only (doc-4): a COMMIT, or a WRITE with a barrier. A store is made between
 * fxfs_device_store_begin and fxfs_device_store_end, and a request that seals is submitted between
 * fxfs_device_seal_begin, which waits for the stores in progress and keeps new ones out, and
 * fxfs_device_seal_end, once the device completes it. Seal_begin then calls the sealer, which
 * makes what may store later, such as user mappings, ask again, and forgets which window pages are
 * writable. Neither stores nor the sealer may wait for the filesystem's Rust, which seals.
 */
typedef void (*fxfs_device_sealer_t)(void *arg);
/* Called once a seal ends; it may not wait either. */
typedef void (*fxfs_device_sealed_t)(void *arg);
void fxfs_device_set_sealer(struct fxfs_device *dev, fxfs_device_sealer_t sealer,
			    fxfs_device_sealed_t sealed, void *arg);

/*
 * The store's capacity, and the shrink the guest is asked to take, or 0, as the device's
 * configuration says now: 0, or -EOPNOTSUPP if it says neither (VIRTIO_EXTENT_F_RESIZE).
 */
int fxfs_device_capacity(struct fxfs_device *dev, u64 *capacity, u64 *asked);
/*
 * Has `resized(arg)` called as the host resizes the store, or a shrink is asked, or, a follower's
 * (FXFS_DEVICE_READ_ONLY), its writer commits; or nothing, if `resized` is NULL. Called from the
 * device's interrupt: it must not sleep.
 */
typedef void (*fxfs_device_resized_t)(void *arg);
void fxfs_device_set_resized(struct fxfs_device *dev, fxfs_device_resized_t resized, void *arg);
/* fxdax: a follower's device's writer's latest commit, as its configuration says now. */
u64 fxfs_device_committed(struct fxfs_device *dev);
/*
 * Waits for a seal in progress to end, and enters the gate. Returns the seal count (as
 * fxfs_device_seals): extents found open when it was the same are still.
 */
u64 fxfs_device_store_begin(struct fxfs_device *dev);
/*
 * Enters the gate, setting `seals`, unless a seal is in progress: false then, and the caller may
 * wait for it to end (fxfs_device_wait_unsealed), having let go of what a store in the gate may
 * wait for, such as a DAX entry a fault holds.
 */
bool fxfs_device_store_try_begin(struct fxfs_device *dev, u64 *seals);
void fxfs_device_wait_unsealed(struct fxfs_device *dev);
void fxfs_device_store_end(struct fxfs_device *dev);
/* fxdax: with lockdep, warns unless this task is in the gate (TASK-44.9). */
void fxfs_device_assert_in_gate(struct fxfs_device *dev);
/*
 * The seal count: how many seals have begun and ended, odd while one is in progress, whose request
 * may seal extents opened meanwhile. The gate never returns an odd count.
 */
u64 fxfs_device_seals(struct fxfs_device *dev);
void fxfs_device_seal_begin(struct fxfs_device *dev);
void fxfs_device_seal_end(struct fxfs_device *dev);

struct dax_device;

/*
 * The DAX window's dax_device, if files' data can be mapped from it: the device maps open extents
 * writably (DAX_WRITE), or, a follower's, maps sealed ones alone, at the page size, which is a
 * block. Sets `window_len`.
 */
struct dax_device *fxfs_device_dax(struct fxfs_device *dev, u64 *window_len);
/*
 * Maps `len` bytes at `offset` in the extent {branch, sequence} at `window_offset` in the window,
 * writably if `writable` (MAP), or unmaps `len` bytes there (UNMAP), and waits for the device.
 */
int fxfs_device_map(struct fxfs_device *dev, u64 branch, u64 sequence, u64 offset, u64 len,
		    u64 window_offset, bool writable);

/* fxdax: one of fxfs_device_map_many's maps, as fxfs_device_map's arguments say it. */
struct fxfs_device_map {
	u64 branch;
	u64 sequence;
	u64 offset;
	u64 len;
	u64 window_offset;
	bool writable;
};

/*
 * fxdax: the `count` maps `maps`, in as few requests as the device takes (MAP_MANY), or one each:
 * a chunk's pieces (TASK-91.4). The first error fails them, whichever others the device mapped.
 */
int fxfs_device_map_many(struct fxfs_device *dev, const struct fxfs_device_map *maps,
			 size_t count);
int fxfs_device_unmap(struct fxfs_device *dev, u64 window_offset, u64 len);
/*
 * fxdax: shows what the window's requests took, and the stores seals held off, as lines of a name
 * and a number (TASK-91.7).
 */
void fxfs_device_show_stats(struct fxfs_device *dev, struct seq_file *m);

#endif
