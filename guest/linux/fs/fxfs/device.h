/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The extent-store device the filesystem lives on (drivers/virtio_extent, backlog doc-4), as the
 * filesystem's Rust uses it: its requests, with plain arguments, completed through a callback.
 * Rust declares the same in guest/rust/fxfs-linux's device module.
 */
#ifndef _FS_FXFS_DEVICE_H
#define _FS_FXFS_DEVICE_H

#include <linux/types.h>

/* A bound extent-store device. */
struct fxfs_device;

/* A WRITE may carry VIRTIO_EXTENT_WRITE_COMMIT. */
#define FXFS_DEVICE_WRITE_COMMIT (1 << 0)

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
 * A request: its type (VIRTIO_EXTENT_T_*) and flags, the extent it names (READ, WRITE, FREE), and
 * where in it (READ, WRITE), and its data: READ's buffer, WRITE's data, COMMIT's root, ROOT's
 * buffer (max_root bytes, and a struct virtio_extent_root_len after them), or INFO's struct
 * virtio_extent_info. The data must be kmalloc'd or vmalloc'd, and stay until the request is done.
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
 * writably (DAX_WRITE), at the page size, which is a block. Sets `window_len`.
 */
struct dax_device *fxfs_device_dax(struct fxfs_device *dev, u64 *window_len);
/*
 * Maps `len` bytes at `offset` in the extent {branch, sequence} at `window_offset` in the window,
 * writably if `writable` (MAP), or unmaps `len` bytes there (UNMAP), and waits for the device.
 */
int fxfs_device_map(struct fxfs_device *dev, u64 branch, u64 sequence, u64 offset, u64 len,
		    u64 window_offset, bool writable);
int fxfs_device_unmap(struct fxfs_device *dev, u64 window_offset, u64 len);

#endif
