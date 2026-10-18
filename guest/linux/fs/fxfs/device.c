// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The extent-store device, as the filesystem uses it (see device.h): requests of the driver's,
 * each allocated as it is submitted and freed as it completes, and the gate between stores
 * through the DAX window's writable mappings and the requests that seal what they map.
 */

#include <linux/minmax.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/virtio_extent.h>
#include <linux/wait.h>

#include "device.h"

struct fxfs_device {
	struct virtio_extent *vext;
	/* Guards the gate. */
	spinlock_t lock;
	/* Stores in progress. */
	unsigned int stores;
	/* Whether a seal is in progress, keeping stores out. */
	bool sealing;
	/* How many seals have begun and ended: odd while one is in progress. */
	u64 seals;
	/* Woken as stores end and seals do. */
	wait_queue_head_t wait;
	fxfs_device_sealer_t sealer;
	fxfs_device_sealed_t sealed;
	void *sealer_arg;
};

struct fxfs_device_request {
	struct vext_request req;
	fxfs_device_done_t done;
	void *arg;
};

struct fxfs_device *fxfs_device_find(const char *tag, size_t len, struct fxfs_device_info *info)
{
	const struct vext_config *config;
	struct virtio_extent *vext;
	struct fxfs_device *dev;
	char name[sizeof(config->tag)];
	u32 max_io;

	if (len >= sizeof(name))
		return NULL;
	memcpy(name, tag, len);
	name[len] = 0;
	vext = virtio_extent_find(name);
	if (!vext)
		return NULL;
	dev = kzalloc(sizeof(*dev), GFP_NOFS);
	if (!dev)
		return NULL;
	dev->vext = vext;
	spin_lock_init(&dev->lock);
	init_waitqueue_head(&dev->wait);
	config = virtio_extent_config(vext);
	/* Memory not aligned to pages spans one more page than its length needs. */
	max_io = min_t(u64, config->max_transfer, (u64)(config->max_segments - 1) * PAGE_SIZE);
	*info = (struct fxfs_device_info) {
		.block_size = config->block_size,
		.max_io = rounddown(max_io, config->block_size),
		.max_root = config->max_root,
		.flags = (virtio_extent_write_commit(vext) ? FXFS_DEVICE_WRITE_COMMIT : 0) |
			 (virtio_extent_freeze(vext) ? FXFS_DEVICE_FREEZE : 0) |
			 (virtio_extent_read_only(vext) ? FXFS_DEVICE_READ_ONLY : 0),
		.branch = config->branch,
		.max_extent = config->max_extent,
	};
	return dev;
}

void fxfs_device_put(struct fxfs_device *dev)
{
	kfree(dev);
}

static void fxfs_device_done(struct vext_request *req)
{
	struct fxfs_device_request *r = container_of(req, struct fxfs_device_request, req);

	r->done(r->arg, req->status);
	kfree(r);
}

int fxfs_device_submit(struct fxfs_device *dev, const struct fxfs_device_op *op,
		       fxfs_device_done_t done, void *arg)
{
	struct fxfs_device_request *r = kzalloc(sizeof(*r), GFP_NOFS);
	int err;

	if (!r)
		return -ENOMEM;
	r->req.type = op->type;
	r->req.flags = op->flags;
	r->req.id = (struct vext_id) { op->branch, op->sequence };
	r->req.offset = op->offset;
	r->req.len = op->len;
	r->req.data = op->data;
	r->req.data_len = op->len;
	r->req.done = fxfs_device_done;
	r->done = done;
	r->arg = arg;
	err = virtio_extent_submit(dev->vext, &r->req);
	if (err)
		kfree(r);
	return err;
}

void fxfs_device_set_sealer(struct fxfs_device *dev, fxfs_device_sealer_t sealer,
			    fxfs_device_sealed_t sealed, void *arg)
{
	spin_lock(&dev->lock);
	dev->sealer = sealer;
	dev->sealed = sealed;
	dev->sealer_arg = arg;
	spin_unlock(&dev->lock);
}

int fxfs_device_capacity(struct fxfs_device *dev, u64 *capacity, u64 *asked)
{
	return virtio_extent_capacity(dev->vext, capacity, asked) ? 0 : -EOPNOTSUPP;
}

void fxfs_device_set_resized(struct fxfs_device *dev, fxfs_device_resized_t resized, void *arg)
{
	virtio_extent_set_config_changed(dev->vext, resized, arg);
}

u64 fxfs_device_committed(struct fxfs_device *dev)
{
	return virtio_extent_committed(dev->vext);
}

u64 fxfs_device_store_begin(struct fxfs_device *dev)
{
	u64 seals;

	spin_lock(&dev->lock);
	while (dev->sealing) {
		spin_unlock(&dev->lock);
		wait_event(dev->wait, !READ_ONCE(dev->sealing));
		spin_lock(&dev->lock);
	}
	dev->stores++;
	seals = dev->seals;
	spin_unlock(&dev->lock);
	return seals;
}

bool fxfs_device_store_try_begin(struct fxfs_device *dev, u64 *seals)
{
	bool entered;

	spin_lock(&dev->lock);
	entered = !dev->sealing;
	if (entered) {
		dev->stores++;
		*seals = dev->seals;
	}
	spin_unlock(&dev->lock);
	return entered;
}

void fxfs_device_wait_unsealed(struct fxfs_device *dev)
{
	wait_event(dev->wait, !READ_ONCE(dev->sealing));
}

void fxfs_device_store_end(struct fxfs_device *dev)
{
	bool last;

	spin_lock(&dev->lock);
	last = !--dev->stores && dev->sealing;
	spin_unlock(&dev->lock);
	if (last)
		wake_up_all(&dev->wait);
}

u64 fxfs_device_seals(struct fxfs_device *dev)
{
	u64 seals;

	spin_lock(&dev->lock);
	seals = dev->seals;
	spin_unlock(&dev->lock);
	return seals;
}

void fxfs_device_seal_begin(struct fxfs_device *dev)
{
	fxfs_device_sealer_t sealer;
	void *arg;

	spin_lock(&dev->lock);
	while (dev->sealing) {
		spin_unlock(&dev->lock);
		wait_event(dev->wait, !READ_ONCE(dev->sealing));
		spin_lock(&dev->lock);
	}
	dev->sealing = true;
	dev->seals++;
	sealer = dev->sealer;
	arg = dev->sealer_arg;
	spin_unlock(&dev->lock);
	wait_event(dev->wait, !READ_ONCE(dev->stores));
	if (sealer)
		sealer(arg);
}

void fxfs_device_seal_end(struct fxfs_device *dev)
{
	fxfs_device_sealed_t sealed;
	void *arg;

	spin_lock(&dev->lock);
	dev->sealing = false;
	dev->seals++;
	sealed = dev->sealed;
	arg = dev->sealer_arg;
	spin_unlock(&dev->lock);
	wake_up_all(&dev->wait);
	if (sealed)
		sealed(arg);
}

struct dax_device *fxfs_device_dax(struct fxfs_device *dev, u64 *window_len)
{
	const struct vext_config *config = virtio_extent_config(dev->vext);

	/* A follower maps sealed extents alone, read-only. */
	bool maps = virtio_extent_dax_write(dev->vext) || virtio_extent_read_only(dev->vext);

	if (!maps || config->block_size != PAGE_SIZE ||
	    config->map_alignment > PAGE_SIZE || !virtio_extent_window(dev->vext, window_len))
		return NULL;
	return virtio_extent_dax(dev->vext);
}

int fxfs_device_map(struct fxfs_device *dev, u64 branch, u64 sequence, u64 offset, u64 len,
		    u64 window_offset, bool writable)
{
	struct vext_request *req = kzalloc(sizeof(*req), GFP_NOFS);
	int err;

	if (!req)
		return -ENOMEM;
	req->type = VIRTIO_EXTENT_T_MAP;
	req->flags = writable ? VIRTIO_EXTENT_MAP_WRITE : 0;
	req->id = (struct vext_id) { branch, sequence };
	req->offset = offset;
	req->len = len;
	req->window_offset = window_offset;
	err = virtio_extent_call(dev->vext, req);
	kfree(req);
	return err;
}

int fxfs_device_unmap(struct fxfs_device *dev, u64 window_offset, u64 len)
{
	struct vext_request *req = kzalloc(sizeof(*req), GFP_NOFS);
	int err;

	if (!req)
		return -ENOMEM;
	req->type = VIRTIO_EXTENT_T_UNMAP;
	req->len = len;
	req->window_offset = window_offset;
	err = virtio_extent_call(dev->vext, req);
	kfree(req);
	return err;
}
