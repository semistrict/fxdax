// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
/*
 * The virtio extent-store device's driver (fxdax's backlog doc-4; include/linux/virtio_extent.h).
 *
 * Requests go to the device's one request queue as descriptor chains: the header and arguments,
 * the data the device reads, the data it writes, and the response. Many may be in flight; the
 * queue's interrupt completes them, calling each one's `done`. A submitter waits for room when
 * the queue is full. The DAX window, if the device has one, is memremapped, so that the guest's
 * Fxfs reads extents mapped into it in place.
 */

#include <linux/completion.h>
#include <linux/io.h>
#include <linux/list.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/virtio.h>
#include <linux/virtio_config.h>
#include <linux/virtio_extent.h>
#include <linux/vmalloc.h>
#include <linux/wait.h>

#include "selftest.h"

struct virtio_extent {
	struct virtio_device *vdev;
	struct virtqueue *vq;
	/* Serializes the queue's use. */
	spinlock_t lock;
	/* Woken as the device completes requests, making room in the queue. */
	wait_queue_head_t room;
	struct vext_config config;
	void *window;
	u64 window_len;
	atomic64_t unique;
	struct list_head node;
};

/* The bound devices. */
static LIST_HEAD(devices);
static DEFINE_MUTEX(devices_lock);

struct virtio_extent *virtio_extent_find(const char *tag)
{
	struct virtio_extent *vext, *found = NULL;

	mutex_lock(&devices_lock);
	list_for_each_entry(vext, &devices, node) {
		if (!strcmp(vext->config.tag, tag)) {
			found = vext;
			break;
		}
	}
	mutex_unlock(&devices_lock);
	return found;
}
EXPORT_SYMBOL_GPL(virtio_extent_find);

const struct vext_config *virtio_extent_config(struct virtio_extent *vext)
{
	return &vext->config;
}
EXPORT_SYMBOL_GPL(virtio_extent_config);

void *virtio_extent_window(struct virtio_extent *vext, u64 *len)
{
	*len = vext->window_len;
	return vext->window;
}
EXPORT_SYMBOL_GPL(virtio_extent_window);

/* Describes `len` bytes of `buf`, kmalloc'd or vmalloc'd, in `sgt`, a page at a time if need be. */
static int vext_map_data(struct sg_table *sgt, void *buf, size_t len)
{
	struct scatterlist *sg;
	unsigned int pages, i;
	size_t offset;
	int err;

	if (!is_vmalloc_addr(buf)) {
		err = sg_alloc_table(sgt, 1, GFP_KERNEL);
		if (!err)
			sg_init_one(sgt->sgl, buf, len);
		return err;
	}
	offset = offset_in_page(buf);
	pages = DIV_ROUND_UP(offset + len, PAGE_SIZE);
	err = sg_alloc_table(sgt, pages, GFP_KERNEL);
	if (err)
		return err;
	for_each_sg(sgt->sgl, sg, pages, i) {
		size_t chunk = min_t(size_t, PAGE_SIZE - offset, len);

		sg_set_page(sg, vmalloc_to_page(buf), chunk, offset);
		buf += chunk;
		len -= chunk;
		offset = 0;
	}
	return 0;
}

/* The arguments' length for a request of `type`, or a -errno for an unknown type. */
static int vext_args(struct vext_request *req)
{
	switch (req->type) {
	case VIRTIO_EXTENT_T_READ:
	case VIRTIO_EXTENT_T_WRITE:
		req->args.io = (struct virtio_extent_io) {
			.id = { cpu_to_le64(req->id.branch), cpu_to_le64(req->id.sequence) },
			.offset = cpu_to_le64(req->offset),
			.len = cpu_to_le32(req->len),
		};
		return sizeof(req->args.io);
	case VIRTIO_EXTENT_T_FREE:
		req->args.id = (struct virtio_extent_id) {
			cpu_to_le64(req->id.branch), cpu_to_le64(req->id.sequence)
		};
		return sizeof(req->args.id);
	case VIRTIO_EXTENT_T_MAP:
		req->args.map = (struct virtio_extent_map) {
			.id = { cpu_to_le64(req->id.branch), cpu_to_le64(req->id.sequence) },
			.offset = cpu_to_le64(req->offset),
			.len = cpu_to_le64(req->len),
			.window_offset = cpu_to_le64(req->window_offset),
		};
		return sizeof(req->args.map);
	case VIRTIO_EXTENT_T_UNMAP:
		req->args.unmap = (struct virtio_extent_unmap) {
			.window_offset = cpu_to_le64(req->window_offset),
			.len = cpu_to_le64(req->len),
		};
		return sizeof(req->args.unmap);
	case VIRTIO_EXTENT_T_COMMIT:
		req->args.commit = (struct virtio_extent_commit) {
			.root_len = cpu_to_le32(req->flags & VIRTIO_EXTENT_COMMIT_ROOT ?
							req->data_len : 0),
		};
		return sizeof(req->args.commit);
	case VIRTIO_EXTENT_T_ROOT:
	case VIRTIO_EXTENT_T_INFO:
		return 0;
	default:
		return -EINVAL;
	}
}

/* The length of the data the device reads (`out`) or writes for `req`. */
static void vext_data_len(struct vext_request *req, size_t *out, size_t *in)
{
	*out = 0;
	*in = 0;
	switch (req->type) {
	case VIRTIO_EXTENT_T_READ:
		*in = req->len;
		break;
	case VIRTIO_EXTENT_T_WRITE:
		*out = req->len;
		break;
	case VIRTIO_EXTENT_T_COMMIT:
		if (req->flags & VIRTIO_EXTENT_COMMIT_ROOT)
			*out = req->data_len;
		break;
	case VIRTIO_EXTENT_T_ROOT:
	case VIRTIO_EXTENT_T_INFO:
		*in = req->data_len;
		break;
	}
}

int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req)
{
	struct scatterlist hdr, args, resp, *sgs[4];
	unsigned int out_sgs = 0, in_sgs = 0;
	size_t out_len, in_len;
	unsigned long flags;
	int args_len, err;

	args_len = vext_args(req);
	if (args_len < 0)
		return args_len;
	vext_data_len(req, &out_len, &in_len);
	if (out_len + in_len > 0 && !req->data)
		return -EINVAL;
	req->hdr = (struct virtio_extent_req) {
		.type = cpu_to_le32(req->type),
		.flags = cpu_to_le32(req->flags),
		.unique = cpu_to_le64(atomic64_inc_return(&vext->unique)),
	};
	req->resp.status = 0;
	memset(&req->data_sgt, 0, sizeof(req->data_sgt));

	sg_init_one(&hdr, &req->hdr, sizeof(req->hdr));
	sgs[out_sgs++] = &hdr;
	if (args_len) {
		sg_init_one(&args, &req->args, args_len);
		sgs[out_sgs++] = &args;
	}
	if (out_len + in_len > 0) {
		err = vext_map_data(&req->data_sgt, req->data, out_len + in_len);
		if (err)
			return err;
		if (out_len)
			sgs[out_sgs++] = req->data_sgt.sgl;
		else
			sgs[out_sgs + in_sgs++] = req->data_sgt.sgl;
	}
	sg_init_one(&resp, &req->resp, sizeof(req->resp));
	sgs[out_sgs + in_sgs++] = &resp;

	for (;;) {
		spin_lock_irqsave(&vext->lock, flags);
		err = virtqueue_add_sgs(vext->vq, sgs, out_sgs, in_sgs, req, GFP_ATOMIC);
		if (!err) {
			bool notify = virtqueue_kick_prepare(vext->vq);

			spin_unlock_irqrestore(&vext->lock, flags);
			if (notify)
				virtqueue_notify(vext->vq);
			return 0;
		}
		spin_unlock_irqrestore(&vext->lock, flags);
		if (err != -ENOSPC) {
			sg_free_table(&req->data_sgt);
			return err;
		}
		/* The queue is full: wait for the device to complete a request. */
		wait_event(vext->room, vext->vq->num_free >= out_sgs + in_sgs);
	}
}
EXPORT_SYMBOL_GPL(virtio_extent_submit);

static void vext_call_done(struct vext_request *req)
{
	complete(req->private);
}

int virtio_extent_call(struct virtio_extent *vext, struct vext_request *req)
{
	DECLARE_COMPLETION_ONSTACK(done);
	int err;

	req->done = vext_call_done;
	req->private = &done;
	err = virtio_extent_submit(vext, req);
	if (err)
		return err;
	wait_for_completion(&done);
	return req->status;
}
EXPORT_SYMBOL_GPL(virtio_extent_call);

/* The queue's interrupt: completes the requests the device has. */
static void vext_done(struct virtqueue *vq)
{
	struct virtio_extent *vext = vq->vdev->priv;
	struct vext_request *req;
	unsigned long flags;
	unsigned int len;

	spin_lock_irqsave(&vext->lock, flags);
	do {
		virtqueue_disable_cb(vq);
		while ((req = virtqueue_get_buf(vq, &len))) {
			req->status = -(int)le32_to_cpu(req->resp.status);
			sg_free_table(&req->data_sgt);
			req->done(req);
		}
	} while (!virtqueue_enable_cb(vq));
	spin_unlock_irqrestore(&vext->lock, flags);
	wake_up_all(&vext->room);
}

static void vext_read_config(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	struct vext_config *config = &vext->config;

	virtio_cread_bytes(vdev, offsetof(struct virtio_extent_config, tag), config->tag,
			   sizeof(config->tag) - 1);
	config->tag[sizeof(config->tag) - 1] = 0;
	virtio_cread_le(vdev, struct virtio_extent_config, block_size, &config->block_size);
	virtio_cread_le(vdev, struct virtio_extent_config, map_alignment, &config->map_alignment);
	virtio_cread_le(vdev, struct virtio_extent_config, max_transfer, &config->max_transfer);
	virtio_cread_le(vdev, struct virtio_extent_config, max_root, &config->max_root);
	virtio_cread_le(vdev, struct virtio_extent_config, branch, &config->branch);
	virtio_cread_le(vdev, struct virtio_extent_config, max_extent, &config->max_extent);
}

/* Maps the DAX window, if the device has one. */
static int vext_map_window(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	struct virtio_shm_region region;

	if (!virtio_has_feature(vdev, VIRTIO_EXTENT_F_DAX))
		return 0;
	if (!virtio_get_shm_region(vdev, &region, 0)) {
		dev_err(&vdev->dev, "the device offers DAX but has no shared memory region\n");
		return -ENODEV;
	}
	if (!devm_request_mem_region(&vdev->dev, region.addr, region.len,
				     dev_name(&vdev->dev))) {
		dev_err(&vdev->dev, "the DAX window's memory is in use\n");
		return -EBUSY;
	}
	vext->window = devm_memremap(&vdev->dev, region.addr, region.len, MEMREMAP_WB);
	if (IS_ERR(vext->window))
		return PTR_ERR(vext->window);
	vext->window_len = region.len;
	return 0;
}

static int vext_probe(struct virtio_device *vdev)
{
	struct virtio_extent *vext;
	int err;

	vext = devm_kzalloc(&vdev->dev, sizeof(*vext), GFP_KERNEL);
	if (!vext)
		return -ENOMEM;
	vext->vdev = vdev;
	vdev->priv = vext;
	spin_lock_init(&vext->lock);
	init_waitqueue_head(&vext->room);
	vext_read_config(vext);
	err = vext_map_window(vext);
	if (err)
		return err;
	vext->vq = virtio_find_single_vq(vdev, vext_done, "requests");
	if (IS_ERR(vext->vq))
		return PTR_ERR(vext->vq);
	virtio_device_ready(vdev);

	err = vext_selftest_add(vext, vdev);
	if (err) {
		virtio_reset_device(vdev);
		vdev->config->del_vqs(vdev);
		return err;
	}
	mutex_lock(&devices_lock);
	list_add(&vext->node, &devices);
	mutex_unlock(&devices_lock);
	dev_info(&vdev->dev, "extent store \"%s\", branch %llx%s\n", vext->config.tag,
		 vext->config.branch, vext->window ? ", with a DAX window" : "");
	return 0;
}

static void vext_remove(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;

	mutex_lock(&devices_lock);
	list_del(&vext->node);
	mutex_unlock(&devices_lock);
	vext_selftest_remove(vdev);
	virtio_reset_device(vdev);
	vdev->config->del_vqs(vdev);
}

static const struct virtio_device_id vext_ids[] = {
	{ VIRTIO_ID_EXTENT, VIRTIO_DEV_ANY_ID },
	{ 0 },
};

static const unsigned int vext_features[] = {
	VIRTIO_EXTENT_F_DAX,
};

static struct virtio_driver vext_driver = {
	.driver.name = "virtio_extent",
	.id_table = vext_ids,
	.feature_table = vext_features,
	.feature_table_size = ARRAY_SIZE(vext_features),
	.probe = vext_probe,
	.remove = vext_remove,
};

module_virtio_driver(vext_driver);
MODULE_DEVICE_TABLE(virtio, vext_ids);
MODULE_DESCRIPTION("The virtio extent-store device's driver");
MODULE_LICENSE("Dual BSD/GPL");
