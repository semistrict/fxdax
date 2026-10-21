// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The virtio extent-store device's driver (fxdax's backlog doc-4; include/linux/virtio_extent.h).
 *
 * Requests go to the device's one request queue as descriptor chains: the header and arguments,
 * the data the device reads, the data it writes, and the response. Many may be in flight; the
 * queue's interrupt completes them, calling each one's `done`. A submitter waits for room when
 * the queue is full. The DAX window, if the device has one, is memremapped, so that the guest's
 * Fxfs reads extents mapped into it in place: as pages of a dax_device, with FS_DAX, as
 * virtio_fs_setup_dax makes its cache window, which files map (fs/fxfs/dax.c).
 */

#include <linux/completion.h>
#include <linux/dax.h>
#include <linux/io.h>
#include <linux/memremap.h>
#include <linux/list.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/sched.h>
#include <linux/sched/clock.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/virtio.h>
#include <linux/virtio_config.h>
#include <linux/virtio_extent.h>
#include <linux/vmalloc.h>
#include <linux/wait.h>

#include "selftest.h"

/* A request queue (fxdax TASK-26.18.5). */
struct vext_queue {
	struct virtqueue *vq;
	/* Serializes the queue's use. */
	spinlock_t lock;
	/* Woken as the device completes requests, making room in the queue. */
	wait_queue_head_t room;
	/* The requests it has taken. */
	atomic64_t requests;
	/*
	 * fxdax: the callers polling it for their requests, while it has any of which the device
	 * signals no completions (vext_wait). Guarded by `lock`.
	 */
	unsigned int pollers;
	/*
	 * fxdax: its interrupts, and the requests it completed, which those callers did without one:
	 * each interrupt costs exits to read and acknowledge it (TASK-91.15).
	 */
	atomic64_t interrupts, polled;
} ____cacheline_aligned_in_smp;

struct virtio_extent {
	struct virtio_device *vdev;
	/*
	 * Its request queues: one per CPU, up to as many as the device has (num_request_queues), each
	 * CPU's requests going to its own, so that CPUs don't contend for one.
	 */
	struct vext_queue *queues;
	unsigned int nr_queues;
	struct vext_config config;
	void *window;
	phys_addr_t window_phys;
	u64 window_len;
	/* The window's dax_device, with FS_DAX. */
	struct dax_device *dax;
	atomic64_t unique;
	/* The requests the device has taken, by type. */
	atomic64_t requests[VIRTIO_EXTENT_T_LAST + 1];
	/* The time the requests it has completed took, queued to completed, by type. */
	atomic64_t request_ns[VIRTIO_EXTENT_T_LAST + 1];
	/*
	 * fxdax: the time recent requests of each type took, a moving average, to wait for one on
	 * the CPU only if it takes less than that is worth (vext_wait).
	 */
	u64 recent_ns[VIRTIO_EXTENT_T_LAST + 1];
	/* What its configuration's changes go to (virtio_extent_set_config_changed). */
	spinlock_t changed_lock;
	vext_config_changed_t changed;
	void *changed_arg;
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

struct dax_device *virtio_extent_dax(struct virtio_extent *vext)
{
	return vext->dax;
}
EXPORT_SYMBOL_GPL(virtio_extent_dax);

bool virtio_extent_dax_write(struct virtio_extent *vext)
{
	return vext->window && virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_DAX_WRITE);
}
EXPORT_SYMBOL_GPL(virtio_extent_dax_write);

bool virtio_extent_write_commit(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_WRITE_COMMIT);
}
EXPORT_SYMBOL_GPL(virtio_extent_write_commit);

bool virtio_extent_freeze(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_FREEZE);
}
EXPORT_SYMBOL_GPL(virtio_extent_freeze);

bool virtio_extent_free_many(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_FREE_MANY);
}
EXPORT_SYMBOL_GPL(virtio_extent_free_many);

bool virtio_extent_map_many(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_MAP_MANY);
}
EXPORT_SYMBOL_GPL(virtio_extent_map_many);

bool virtio_extent_read_only(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_READ_ONLY);
}
EXPORT_SYMBOL_GPL(virtio_extent_read_only);

/* Whether `req` would write a read-only device's store, which it refuses. */
static bool vext_writes(const struct vext_request *req)
{
	switch (req->type) {
	case VIRTIO_EXTENT_T_WRITE:
	case VIRTIO_EXTENT_T_FREE:
	case VIRTIO_EXTENT_T_COMMIT:
	case VIRTIO_EXTENT_T_RESIZED:
		return true;
	case VIRTIO_EXTENT_T_MAP:
		return req->flags & VIRTIO_EXTENT_MAP_WRITE;
	default:
		return false;
	}
}

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
	case VIRTIO_EXTENT_T_PIN:
	case VIRTIO_EXTENT_T_UNPIN:
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
	case VIRTIO_EXTENT_T_FREEZE:
	case VIRTIO_EXTENT_T_ADVANCE:
		return 0;
	case VIRTIO_EXTENT_T_LET_GO:
		req->args.epoch = (struct virtio_extent_epoch) {
			.epoch = cpu_to_le64(req->offset),
		};
		return sizeof(req->args.epoch);
	case VIRTIO_EXTENT_T_RESIZED:
		req->args.resized = (struct virtio_extent_resized) {
			.capacity = cpu_to_le64(req->offset),
		};
		return sizeof(req->args.resized);
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
	case VIRTIO_EXTENT_T_FREE:
		if (req->flags & VIRTIO_EXTENT_FREE_MANY)
			*out = req->data_len;
		break;
	case VIRTIO_EXTENT_T_MAP:
		if (req->flags & VIRTIO_EXTENT_MAP_MANY)
			*out = req->data_len;
		break;
	case VIRTIO_EXTENT_T_ROOT:
	case VIRTIO_EXTENT_T_INFO:
	case VIRTIO_EXTENT_T_ADVANCE:
		*in = req->data_len;
		break;
	}
}

int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req)
{
	struct scatterlist hdr, args, resp, *sgs[4];
	unsigned int out_sgs = 0, in_sgs = 0, descs;
	struct vext_queue *queue;
	size_t out_len, in_len;
	unsigned long flags;
	int args_len, err;

	/* fxdax: a follower's filesystem writes nothing: a write here is its bug (doc-8, testing 1). */
	if (WARN_ON_ONCE(virtio_extent_read_only(vext) && vext_writes(req)))
		return -EROFS;
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

	/* A descriptor per scatterlist entry: the queue has no indirect ones. */
	descs = out_sgs + in_sgs + (req->data_sgt.nents ? req->data_sgt.nents - 1 : 0);
	if (req->data_sgt.nents > vext->config.max_segments) {
		sg_free_table(&req->data_sgt);
		return -EINVAL;
	}

	/* The queue of the CPU it runs on: if it moves meanwhile, another's, which is as good. */
	queue = &vext->queues[raw_smp_processor_id() % vext->nr_queues];
	for (;;) {
		req->queued_ns = ktime_get_ns();
		spin_lock_irqsave(&queue->lock, flags);
		err = virtqueue_add_sgs(queue->vq, sgs, out_sgs, in_sgs, req, GFP_ATOMIC);
		if (!err) {
			bool notify = virtqueue_kick_prepare(queue->vq);

			atomic64_inc(&vext->requests[req->type]);
			atomic64_inc(&queue->requests);
			req->queue = queue;
			spin_unlock_irqrestore(&queue->lock, flags);
			if (notify)
				virtqueue_notify(queue->vq);
			return 0;
		}
		spin_unlock_irqrestore(&queue->lock, flags);
		if (err != -ENOSPC) {
			sg_free_table(&req->data_sgt);
			return err;
		}
		/* The queue is full: wait for the device to complete a request. */
		wait_event(queue->room, queue->vq->num_free >= descs);
	}
}
EXPORT_SYMBOL_GPL(virtio_extent_submit);

static void vext_call_done(struct vext_request *req)
{
	complete(req->private);
}

/*
 * Completes the requests the device has done of `queue`, with its lock held: how many there were.
 * The time each took goes to its type's average (vext_wait).
 */
static unsigned int vext_reap(struct virtio_extent *vext, struct vext_queue *queue)
{
	struct vext_request *req;
	unsigned int len, reaped = 0;
	u64 took, *recent;

	while ((req = virtqueue_get_buf(queue->vq, &len))) {
		took = ktime_get_ns() - req->queued_ns;
		atomic64_add(took, &vext->request_ns[req->type]);
		/* An eighth of the latest, as TCP's smoothed round trip time. */
		recent = &vext->recent_ns[req->type];
		WRITE_ONCE(*recent, READ_ONCE(*recent) - (READ_ONCE(*recent) >> 3) + (took >> 3));
		req->status = -(int)le32_to_cpu(req->resp.status);
		sg_free_table(&req->data_sgt);
		req->done(req);
		reaped++;
	}
	return reaped;
}

/*
 * Has the device signal `queue`'s completions again, unless callers poll it, completing those it
 * did meanwhile: with its lock held. How many there were.
 */
static unsigned int vext_signal(struct virtio_extent *vext, struct vext_queue *queue)
{
	unsigned int reaped = 0;

	while (!queue->pollers && !virtqueue_enable_cb(queue->vq)) {
		virtqueue_disable_cb(queue->vq);
		reaped += vext_reap(vext, queue);
	}
	return reaped;
}

/*
 * How long a caller waits for its request on the CPU before sleeping: a request's reply comes
 * back in tens to hundreds of microseconds, and sleeping costs halting the vCPU, which in a nested
 * VM exits to the outer host, and an interrupt to wake it. fxdax: only for requests of a type
 * that recently took less (TASK-91.13): MAPs taking 260-490 us spun their 100 us for nothing,
 * a quarter to half of the guest's time.
 */
#define VEXT_SPIN_NS (100 * NSEC_PER_USEC)

/*
 * Waits for `req`, whose completion is `done`: on the CPU first, if requests of its type
 * recently took less than VEXT_SPIN_NS, and no other task wants the CPU. fxdax: polling its
 * queue meanwhile, of which the device signals no completions: each interrupt costs exits to
 * read and acknowledge it, which a request done in time saves (TASK-91.15).
 */
static void vext_wait(struct virtio_extent *vext, struct vext_request *req, struct completion *done)
{
	struct vext_queue *queue = req->queue;
	u64 deadline = local_clock() + VEXT_SPIN_NS;
	unsigned int spins = 0, reaped;
	unsigned long flags;
	bool completed;

	if (READ_ONCE(vext->recent_ns[req->type]) >= VEXT_SPIN_NS) {
		wait_for_completion(done);
		return;
	}
	/* Not preempted while the device signals no completions of the queue, which it polls. */
	preempt_disable();
	spin_lock_irqsave(&queue->lock, flags);
	if (!queue->pollers++)
		virtqueue_disable_cb(queue->vq);
	spin_unlock_irqrestore(&queue->lock, flags);
	/* Taken here, the completion isn't there for wait_for_completion. */
	while (!(completed = try_wait_for_completion(done))) {
		/* The clock, a paravirtual one, is costly to read: every 64 spins. */
		if (need_resched() || (++spins % 64 == 0 && local_clock() > deadline))
			break;
		/* The used ring, in the guest's memory, every 16. */
		if (spins % 16 == 0 && spin_trylock_irqsave(&queue->lock, flags)) {
			reaped = vext_reap(vext, queue);
			spin_unlock_irqrestore(&queue->lock, flags);
			if (reaped) {
				atomic64_add(reaped, &queue->polled);
				wake_up_all(&queue->room);
			}
		}
		cpu_relax();
	}
	spin_lock_irqsave(&queue->lock, flags);
	queue->pollers--;
	reaped = vext_signal(vext, queue);
	spin_unlock_irqrestore(&queue->lock, flags);
	preempt_enable();
	if (reaped) {
		atomic64_add(reaped, &queue->polled);
		wake_up_all(&queue->room);
	}
	if (!completed)
		wait_for_completion(done);
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
	vext_wait(vext, req, &done);
	return req->status;
}
EXPORT_SYMBOL_GPL(virtio_extent_call);

/*
 * A queue's interrupt: completes the requests the device has of it. fxdax: and has the device
 * signal it again, unless callers poll it (vext_wait).
 */
static void vext_done(struct virtqueue *vq)
{
	struct virtio_extent *vext = vq->vdev->priv;
	struct vext_queue *queue = &vext->queues[vq->index];
	unsigned long flags;

	atomic64_inc(&queue->interrupts);
	spin_lock_irqsave(&queue->lock, flags);
	virtqueue_disable_cb(vq);
	vext_reap(vext, queue);
	vext_signal(vext, queue);
	spin_unlock_irqrestore(&queue->lock, flags);
	wake_up_all(&queue->room);
}

/*
 * Finds the device's request queues, one for each CPU, up to as many as it has: one, if it says
 * none (num_request_queues is 1 at least).
 */
static int vext_find_queues(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	struct virtqueue_info *infos;
	struct virtqueue **vqs;
	unsigned int i, offered;
	int err;

	virtio_cread_le(vdev, struct virtio_extent_config, num_request_queues, &offered);
	vext->nr_queues = clamp(offered, 1U, nr_cpu_ids);
	vext->queues = devm_kcalloc(&vdev->dev, vext->nr_queues, sizeof(*vext->queues), GFP_KERNEL);
	infos = kcalloc(vext->nr_queues, sizeof(*infos), GFP_KERNEL);
	vqs = kcalloc(vext->nr_queues, sizeof(*vqs), GFP_KERNEL);
	err = -ENOMEM;
	if (!vext->queues || !infos || !vqs)
		goto out;
	for (i = 0; i < vext->nr_queues; i++)
		infos[i] = (struct virtqueue_info){ "requests", vext_done };
	err = virtio_find_vqs(vdev, vext->nr_queues, vqs, infos, NULL);
	if (err)
		goto out;
	for (i = 0; i < vext->nr_queues; i++) {
		vext->queues[i].vq = vqs[i];
		spin_lock_init(&vext->queues[i].lock);
		init_waitqueue_head(&vext->queues[i].room);
	}
out:
	kfree(infos);
	kfree(vqs);
	return err;
}

static const char *const request_names[] = {
	[VIRTIO_EXTENT_T_READ] = "read",     [VIRTIO_EXTENT_T_WRITE] = "write",
	[VIRTIO_EXTENT_T_FREE] = "free",     [VIRTIO_EXTENT_T_MAP] = "map",
	[VIRTIO_EXTENT_T_UNMAP] = "unmap",   [VIRTIO_EXTENT_T_COMMIT] = "commit",
	[VIRTIO_EXTENT_T_ROOT] = "root",     [VIRTIO_EXTENT_T_INFO] = "info",
	[VIRTIO_EXTENT_T_RESIZED] = "resized",
	[VIRTIO_EXTENT_T_FREEZE] = "freeze", [VIRTIO_EXTENT_T_ADVANCE] = "advance",
	[VIRTIO_EXTENT_T_LET_GO] = "let_go", [VIRTIO_EXTENT_T_PIN] = "pin",
	[VIRTIO_EXTENT_T_UNPIN] = "unpin",
};

/* Shows `counters`, one per request type, each on a line of its own after the type's name. */
static ssize_t request_counters_show(char *buf, atomic64_t *counters)
{
	ssize_t len = 0;
	int type;

	for (type = VIRTIO_EXTENT_T_READ; type <= VIRTIO_EXTENT_T_LAST; type++)
		len += sysfs_emit_at(buf, len, "%s %lld\n", request_names[type],
				     atomic64_read(&counters[type]));
	return len;
}

/* The `requests` attribute: how many requests of each type the device has taken. */
static ssize_t requests_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;

	return request_counters_show(buf, vext->requests);
}
static DEVICE_ATTR_RO(requests);

/* The `request_ns` attribute: how long the requests of each type completed took, all told. */
static ssize_t request_ns_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;

	return request_counters_show(buf, vext->request_ns);
}
static DEVICE_ATTR_RO(request_ns);

/* The `queues` attribute: how many requests each request queue has taken, a line each. */
static ssize_t queues_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;
	ssize_t len = 0;
	unsigned int i;

	for (i = 0; i < vext->nr_queues; i++)
		len += sysfs_emit_at(buf, len, "%lld\n", atomic64_read(&vext->queues[i].requests));
	return len;
}
static DEVICE_ATTR_RO(queues);

/*
 * fxdax: the `completions` attribute: the queues' interrupts, and the requests their callers
 * completed polling them, without one (vext_wait), all told.
 */
static ssize_t completions_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;
	s64 interrupts = 0, polled = 0;
	unsigned int i;

	for (i = 0; i < vext->nr_queues; i++) {
		interrupts += atomic64_read(&vext->queues[i].interrupts);
		polled += atomic64_read(&vext->queues[i].polled);
	}
	return sysfs_emit(buf, "interrupts %lld\npolled %lld\n", interrupts, polled);
}
static DEVICE_ATTR_RO(completions);

static struct attribute *vext_attrs[] = {
	&dev_attr_requests.attr,
	&dev_attr_request_ns.attr,
	&dev_attr_queues.attr,
	&dev_attr_completions.attr,
	NULL,
};

static const struct attribute_group vext_attr_group = { .attrs = vext_attrs };

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
	virtio_cread_le(vdev, struct virtio_extent_config, max_segments, &config->max_segments);
	virtio_cread_le(vdev, struct virtio_extent_config, max_root, &config->max_root);
	virtio_cread_le(vdev, struct virtio_extent_config, branch, &config->branch);
	virtio_cread_le(vdev, struct virtio_extent_config, max_extent, &config->max_extent);
}

#if IS_ENABLED(CONFIG_FS_DAX)
/* The window's memory at page `pgoff` of it, up to `nr_pages` of it, as virtio_fs's. */
static long vext_dax_direct_access(struct dax_device *dax_dev, pgoff_t pgoff, long nr_pages,
				   enum dax_access_mode mode, void **kaddr, unsigned long *pfn)
{
	struct virtio_extent *vext = dax_get_private(dax_dev);
	phys_addr_t offset = PFN_PHYS(pgoff);
	long max_nr_pages = vext->window_len / PAGE_SIZE - pgoff;

	if (kaddr)
		*kaddr = vext->window + offset;
	if (pfn)
		*pfn = PHYS_PFN(vext->window_phys + offset);
	return min(nr_pages, max_nr_pages);
}

static int vext_dax_zero_page_range(struct dax_device *dax_dev, pgoff_t pgoff, size_t nr_pages)
{
	void *kaddr;
	long rc = dax_direct_access(dax_dev, pgoff, nr_pages, DAX_ACCESS, &kaddr, NULL);

	if (rc < 0)
		return dax_mem2blk_err(rc);
	memset(kaddr, 0, nr_pages << PAGE_SHIFT);
	dax_flush(dax_dev, kaddr, nr_pages << PAGE_SHIFT);
	return 0;
}

static const struct dax_operations vext_dax_ops = {
	.direct_access = vext_dax_direct_access,
	.zero_page_range = vext_dax_zero_page_range,
};

static void vext_put_dax(void *data)
{
	struct dax_device *dax = data;

	kill_dax(dax);
	put_dax(dax);
}

/* Maps the window as pages a dax_device hands out, which mappings of files can map. */
static int vext_map_window_dax(struct virtio_extent *vext, struct virtio_shm_region *region)
{
	struct device *dev = &vext->vdev->dev;
	struct dev_pagemap *pgmap = devm_kzalloc(dev, sizeof(*pgmap), GFP_KERNEL);
	struct dax_device *dax;

	if (!pgmap)
		return -ENOMEM;
	pgmap->type = MEMORY_DEVICE_FS_DAX;
	pgmap->range = (struct range) {
		.start = region->addr,
		.end = region->addr + region->len - 1,
	};
	pgmap->nr_range = 1;
	vext->window = devm_memremap_pages(dev, pgmap);
	if (IS_ERR(vext->window))
		return PTR_ERR(vext->window);
	dax = alloc_dax(vext, &vext_dax_ops);
	if (IS_ERR(dax))
		return PTR_ERR(dax);
	vext->dax = dax;
	return devm_add_action_or_reset(dev, vext_put_dax, dax);
}
#endif

/* Maps the DAX window, if the device has one. */
static int vext_map_window(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	struct virtio_shm_region region;
	int err;

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
	vext->window_phys = region.addr;
	vext->window_len = region.len;
#if IS_ENABLED(CONFIG_FS_DAX)
	err = vext_map_window_dax(vext, &region);
#else
	vext->window = devm_memremap(&vdev->dev, region.addr, region.len, MEMREMAP_WB);
	err = PTR_ERR_OR_ZERO(vext->window);
#endif
	if (err)
		vext->window = NULL;
	return err;
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
	spin_lock_init(&vext->changed_lock);
	vext_read_config(vext);
	err = vext_map_window(vext);
	if (err)
		return err;
	err = vext_find_queues(vext);
	if (err)
		return err;
	/* A request's data pages, beside its header, arguments and response, fit a queue. */
	vext->config.max_segments = min(vext->config.max_segments,
					virtqueue_get_vring_size(vext->queues[0].vq) - 3);
	virtio_device_ready(vdev);

	err = sysfs_create_group(&vdev->dev.kobj, &vext_attr_group);
	if (!err)
		err = vext_selftest_add(vext, vdev);
	if (err) {
		sysfs_remove_group(&vdev->dev.kobj, &vext_attr_group);
		virtio_reset_device(vdev);
		vdev->config->del_vqs(vdev);
		return err;
	}
	mutex_lock(&devices_lock);
	list_add(&vext->node, &devices);
	mutex_unlock(&devices_lock);
	dev_info(&vdev->dev, "extent store \"%s\", branch %llx, %u request queue%s%s%s%s\n",
		 vext->config.tag, vext->config.branch, vext->nr_queues,
		 vext->nr_queues == 1 ? "" : "s", vext->window ? ", with a DAX window" : "",
		 virtio_extent_dax_write(vext) ? " to write through" : "",
		 virtio_extent_read_only(vext) ? ", following its writer read-only" : "");
	return 0;
}

bool virtio_extent_capacity(struct virtio_extent *vext, u64 *capacity, u64 *asked)
{
	struct virtio_device *vdev = vext->vdev;
	u64 again;

	if (!virtio_has_feature(vdev, VIRTIO_EXTENT_F_RESIZE))
		return false;
	/* Each read whole: the same twice, as the device may change it between halves. */
	virtio_cread_le(vdev, struct virtio_extent_config, capacity, capacity);
	do {
		again = *capacity;
		virtio_cread_le(vdev, struct virtio_extent_config, capacity, capacity);
	} while (*capacity != again);
	virtio_cread_le(vdev, struct virtio_extent_config, asked, asked);
	do {
		again = *asked;
		virtio_cread_le(vdev, struct virtio_extent_config, asked, asked);
	} while (*asked != again);
	return true;
}
EXPORT_SYMBOL_GPL(virtio_extent_capacity);

u64 virtio_extent_committed(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	u64 committed, again;

	if (!virtio_extent_read_only(vext))
		return 0;
	/* Read whole: the same twice, as the device may change it between halves. */
	virtio_cread_le(vdev, struct virtio_extent_config, committed, &committed);
	do {
		again = committed;
		virtio_cread_le(vdev, struct virtio_extent_config, committed, &committed);
	} while (committed != again);
	return committed;
}
EXPORT_SYMBOL_GPL(virtio_extent_committed);

void virtio_extent_set_config_changed(struct virtio_extent *vext, vext_config_changed_t changed,
				      void *arg)
{
	unsigned long flags;

	spin_lock_irqsave(&vext->changed_lock, flags);
	vext->changed = changed;
	vext->changed_arg = arg;
	spin_unlock_irqrestore(&vext->changed_lock, flags);
}
EXPORT_SYMBOL_GPL(virtio_extent_set_config_changed);

static void vext_config_changed(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;
	unsigned long flags;

	spin_lock_irqsave(&vext->changed_lock, flags);
	if (vext->changed)
		vext->changed(vext->changed_arg);
	spin_unlock_irqrestore(&vext->changed_lock, flags);
}

static void vext_remove(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;

	mutex_lock(&devices_lock);
	list_del(&vext->node);
	mutex_unlock(&devices_lock);
	vext_selftest_remove(vdev);
	sysfs_remove_group(&vdev->dev.kobj, &vext_attr_group);
	virtio_reset_device(vdev);
	vdev->config->del_vqs(vdev);
}

static const struct virtio_device_id vext_ids[] = {
	{ VIRTIO_ID_EXTENT, VIRTIO_DEV_ANY_ID },
	{ 0 },
};

static const unsigned int vext_features[] = {
	VIRTIO_EXTENT_F_DAX,
	VIRTIO_EXTENT_F_DAX_WRITE,
	VIRTIO_EXTENT_F_WRITE_COMMIT,
	VIRTIO_EXTENT_F_RESIZE,
	VIRTIO_EXTENT_F_FREEZE,
	VIRTIO_EXTENT_F_READ_ONLY,
	VIRTIO_EXTENT_F_FREE_MANY,
	VIRTIO_EXTENT_F_MAP_MANY,
};

static struct virtio_driver vext_driver = {
	.driver.name = "virtio_extent",
	.id_table = vext_ids,
	.feature_table = vext_features,
	.feature_table_size = ARRAY_SIZE(vext_features),
	.probe = vext_probe,
	.remove = vext_remove,
	.config_changed = vext_config_changed,
};

module_virtio_driver(vext_driver);
MODULE_DEVICE_TABLE(virtio, vext_ids);
MODULE_DESCRIPTION("The virtio extent-store device's driver");
MODULE_LICENSE("Dual BSD/GPL");
