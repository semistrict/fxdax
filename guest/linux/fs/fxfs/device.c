// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * The extent-store device, as the filesystem's Rust uses it (see device.h): requests of the
 * driver's, each allocated as it is submitted and freed as it completes.
 */

#include <linux/minmax.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/virtio_extent.h>

#include "device.h"

struct fxfs_device_request {
	struct vext_request req;
	fxfs_device_done_t done;
	void *arg;
};

struct fxfs_device *fxfs_device_find(const char *tag, size_t len, struct fxfs_device_info *info)
{
	const struct vext_config *config;
	struct virtio_extent *vext;
	char name[sizeof(config->tag)];
	u32 max_io;

	if (len >= sizeof(name))
		return NULL;
	memcpy(name, tag, len);
	name[len] = 0;
	vext = virtio_extent_find(name);
	if (!vext)
		return NULL;
	config = virtio_extent_config(vext);
	/* Memory not aligned to pages spans one more page than its length needs. */
	max_io = min_t(u64, config->max_transfer, (u64)(config->max_segments - 1) * PAGE_SIZE);
	*info = (struct fxfs_device_info) {
		.block_size = config->block_size,
		.max_io = rounddown(max_io, config->block_size),
		.max_root = config->max_root,
		.branch = config->branch,
		.max_extent = config->max_extent,
	};
	return (struct fxfs_device *)vext;
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
	err = virtio_extent_submit((struct virtio_extent *)dev, &r->req);
	if (err)
		kfree(r);
	return err;
}
