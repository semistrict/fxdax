/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
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

/* What the filesystem needs to know of a device. */
struct fxfs_device_info {
	u32 block_size;
	/* The most bytes one READ or WRITE moves, from anywhere in memory: a multiple of blocks. */
	u32 max_io;
	/* The most bytes the root holds. */
	u32 max_root;
	u32 reserved;
	/* The branch whose extents the device's filesystem writes. */
	u64 branch;
	/* The most bytes one extent holds. */
	u64 max_extent;
};

/* The device tagged with the `len` bytes of `tag`, filling in `info`, or NULL if there is none. */
struct fxfs_device *fxfs_device_find(const char *tag, size_t len, struct fxfs_device_info *info);

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

#endif
