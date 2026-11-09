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
 * virtio_fs_setup_dax makes its cache window, which files map (fs/fxfs/dax.c). fxdax: without
 * FS_DAX, the driver takes no window: nothing would map it.
 */

#include <linux/completion.h>
#include <linux/dax.h>
#include <linux/device/driver.h>
#include <linux/io.h>
#include <linux/memremap.h>
#include <linux/list.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/sched.h>
#include <linux/sched/clock.h>
#include <linux/sched/mm.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/virtio.h>
#include <linux/virtio_config.h>
#include <linux/virtio_extent.h>
#include <linux/vmalloc.h>
#include <linux/wait.h>
#include <linux/workqueue.h>

#include "selftest.h"

/* A request queue (fxdax TASK-26.18.5). */
struct vext_queue {
	struct virtqueue *vq;
	/* Serializes the queue's use. */
	spinlock_t lock;
	/*
	 * fxdax: whether it takes no requests, as the device resets (vext_reset_prepare) until it
	 * finds its queues again: set, and read before a request is added, under `lock`, so that none
	 * is added to a queue the reset goes on to let go of (TASK-201).
	 */
	bool closed;
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
	/*
	 * fxdax: the requests completed on a CPU other than the one they were queued on: each wakes
	 * its caller with an interrupt between CPUs, and finds its request in another's cache
	 * (TASK-276).
	 */
	atomic64_t cross;
} ____cacheline_aligned_in_smp;

/*
 * fxdax: the device's DAX window, its shared memory region, as the pages of a dax_device, which
 * mappings of files map (fs/fxfs/dax.c).
 *
 * Its struct pages are made a part at a time, as dax_direct_access first reaches a part
 * (TASK-92.8). Made whole as the device was probed, a 2 GiB window's took a third of the guest's
 * boot, and 32 MiB of its memory (64 bytes a page), most of which a guest never maps: Fxfs hands
 * the window's space out first fit, from its start and from its middle (fs/fxfs/dax.c). A part is
 * a memory section of the window, or what of one it covers: hotplug maps a whole section's struct
 * pages on huge pages, and each making waits for an RCU grace period (mem_hotplug_begin).
 * Nothing reaches the window's memory but through dax_direct_access, so no page is used before it
 * has a struct page.
 */
struct vext_window {
	phys_addr_t phys;
	u64 len;
	/* Its memory, mapped in the kernel, where each part's making maps it (the direct map). */
	void *kaddr;
	struct dax_device *dax;
	/* Its parts' pages, the first's section the window's first; their node's. */
	struct dev_pagemap *parts;
	unsigned int nr_parts;
	int nid;
	/* Which parts have struct pages, which `lock` serializes the making of. */
	unsigned long *made_parts;
	struct mutex lock;
	/* How much of it has struct pages, and the time making them took. */
	atomic64_t made, made_ns;
};

struct virtio_extent {
	struct virtio_device *vdev;
	/*
	 * Its request queues: one per CPU, up to as many as the device has (num_request_queues), each
	 * CPU's requests going to its own, so that CPUs don't contend for one.
	 */
	struct vext_queue *queues;
	unsigned int nr_queues;
	struct vext_config config;
	/* fxdax: its DAX window, if it has one (F_DAX, with FS_DAX). */
	struct vext_window window;
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
	/*
	 * fxdax: what answers the host's asks for the guest's stats (virtio_extent_set_stats), which
	 * `stats_lock` guards; the work answering them, from a configuration change; and the ask it
	 * answered last (TASK-111.4).
	 */
	struct mutex stats_lock;
	vext_stats_t stats;
	void *stats_arg;
	struct work_struct stats_work;
	u64 stats_answered;
	/*
	 * fxdax: whether it is closed (virtio_extent_close), which `open_lock` guards: its queues are
	 * then (`struct vext_queue`'s `closed`).
	 */
	struct mutex open_lock;
	bool closed;
	/* fxdax: those who found it and have yet to put it, whom unbinding it waits for. */
	atomic_t users;
	struct list_head node;
};

/* The bound devices. */
static LIST_HEAD(devices);
static DEFINE_MUTEX(devices_lock);
/* fxdax: unbinding, for a device's `users` to go; not the device's, which then goes too. */
static DECLARE_WAIT_QUEUE_HEAD(devices_unused);

static struct virtio_extent *vext_find_bound(const char *tag)
{
	struct virtio_extent *vext, *found = NULL;

	mutex_lock(&devices_lock);
	list_for_each_entry(vext, &devices, node) {
		if (!strcmp(vext->config.tag, tag)) {
			atomic_inc(&vext->users);
			found = vext;
			break;
		}
	}
	mutex_unlock(&devices_lock);
	return found;
}

/*
 * fxdax: the device tagged `tag`, once probes in progress are done if none is bound yet: devices
 * are probed asynchronously (vext_driver), so that the window's memmap, built as a device is
 * probed, a third of a fork's boot, overlaps the rest of the boot (TASK-92.8); a mount by tag
 * waits for its device's probe, as the kernel waits for probes before it mounts the root.
 */
struct virtio_extent *virtio_extent_find(const char *tag)
{
	struct virtio_extent *found = vext_find_bound(tag);

	if (found)
		return found;
	wait_for_device_probe();
	return vext_find_bound(tag);
}
EXPORT_SYMBOL_GPL(virtio_extent_find);

void virtio_extent_put(struct virtio_extent *vext)
{
	if (atomic_dec_and_test(&vext->users))
		wake_up_all(&devices_unused);
}
EXPORT_SYMBOL_GPL(virtio_extent_put);

const struct vext_config *virtio_extent_config(struct virtio_extent *vext)
{
	return &vext->config;
}
EXPORT_SYMBOL_GPL(virtio_extent_config);

struct dax_device *virtio_extent_dax(struct virtio_extent *vext, u64 *window_len)
{
	*window_len = vext->window.len;
	return vext->window.dax;
}
EXPORT_SYMBOL_GPL(virtio_extent_dax);

u64 virtio_extent_private_max(struct virtio_extent *vext)
{
	return vext->config.private_max;
}
EXPORT_SYMBOL_GPL(virtio_extent_private_max);

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

bool virtio_extent_map_ahead(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_MAP_AHEAD);
}
EXPORT_SYMBOL_GPL(virtio_extent_map_ahead);

bool virtio_extent_read_only(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_READ_ONLY);
}
EXPORT_SYMBOL_GPL(virtio_extent_read_only);

/* fxdax: whether the host asks the guest for its stats (F_STATS, TASK-111.4). */
static bool vext_stats_asked(struct virtio_extent *vext)
{
	return virtio_has_feature(vext->vdev, VIRTIO_EXTENT_F_STATS);
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

static struct virtio_extent_id vext_wire_id(struct vext_id id)
{
	return (struct virtio_extent_id) { cpu_to_le64(id.branch), cpu_to_le64(id.sequence) };
}

/*
 * The encoders of requests' arguments, into req->args, given the lengths of the data and the root
 * the request carries: the arguments' length.
 */
static u32 vext_args_io(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.io = (struct virtio_extent_io) {
		.id = vext_wire_id(req->id),
		.offset = cpu_to_le64(req->offset),
		.len = cpu_to_le32(data_len),
		.root_len = cpu_to_le32(root_len),
	};
	return sizeof(req->args.io);
}

static u32 vext_args_id(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.id = vext_wire_id(req->id);
	return sizeof(req->args.id);
}

static u32 vext_args_map(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.map = (struct virtio_extent_map) {
		.id = vext_wire_id(req->id),
		.offset = cpu_to_le64(req->offset),
		.len = cpu_to_le64(req->len),
		.window_offset = cpu_to_le64(req->window_offset),
	};
	return sizeof(req->args.map);
}

static u32 vext_args_unmap(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.unmap = (struct virtio_extent_unmap) {
		.window_offset = cpu_to_le64(req->window_offset),
		.len = cpu_to_le64(req->len),
	};
	return sizeof(req->args.unmap);
}

static u32 vext_args_private(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.private = (struct virtio_extent_private) {
		.window_offset = cpu_to_le64(req->window_offset),
		.len = cpu_to_le64(req->len),
	};
	return sizeof(req->args.private);
}

/* A COMMIT's data is the root it sets. */
static u32 vext_args_commit(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.commit = (struct virtio_extent_commit) { .root_len = cpu_to_le32(data_len) };
	return sizeof(req->args.commit);
}

static u32 vext_args_resized(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.resized = (struct virtio_extent_resized) { .capacity = cpu_to_le64(req->offset) };
	return sizeof(req->args.resized);
}

static u32 vext_args_epoch(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.epoch = (struct virtio_extent_epoch) { .epoch = cpu_to_le64(req->offset) };
	return sizeof(req->args.epoch);
}

/* fxdax: a STATS names the ask it answers (TASK-111.4). */
static u32 vext_args_stats(struct vext_request *req, u32 data_len, u32 root_len)
{
	req->args.stats = (struct virtio_extent_stats) { .asked = cpu_to_le64(req->offset) };
	return sizeof(req->args.stats);
}

/* Which way a request's data goes: to the device, which reads it, or from it, which writes it. */
enum vext_way { VEXT_NO_DATA, VEXT_OUT, VEXT_IN };

/*
 * fxdax: what a request of a type carries after its header, a row per type, its whole wire
 * contract (TASK-160.7): its arguments, which `args` encodes, if it has any; its data, the
 * request's `data_len` bytes, which go `data`'s way, if the request has the flag `data_if`, or
 * always if that is 0; and a root after its data, the request's `root_len` bytes, if it has the
 * flag `root_if`. And its name, its counters' in sysfs, and whether it writes the store, which a
 * read-only device refuses.
 */
static const struct vext_type {
	const char *name;
	u32 (*args)(struct vext_request *req, u32 data_len, u32 root_len);
	enum vext_way data;
	u32 data_if;
	u32 root_if;
	bool writes;
} vext_types[VIRTIO_EXTENT_T_LAST + 1] = {
	[VIRTIO_EXTENT_T_READ] = { "read", vext_args_io, VEXT_IN },
	[VIRTIO_EXTENT_T_WRITE] = { "write", vext_args_io, VEXT_OUT,
				    .root_if = VIRTIO_EXTENT_WRITE_ROOT, .writes = true },
	[VIRTIO_EXTENT_T_FREE] = { "free", vext_args_id, VEXT_OUT, VIRTIO_EXTENT_FREE_MANY,
				   .writes = true },
	[VIRTIO_EXTENT_T_MAP] = { "map", vext_args_map, VEXT_OUT, VIRTIO_EXTENT_MAP_MANY },
	[VIRTIO_EXTENT_T_UNMAP] = { "unmap", vext_args_unmap },
	[VIRTIO_EXTENT_T_COMMIT] = { "commit", vext_args_commit, VEXT_OUT,
				     VIRTIO_EXTENT_COMMIT_ROOT, .writes = true },
	[VIRTIO_EXTENT_T_ROOT] = { "root", NULL, VEXT_IN },
	[VIRTIO_EXTENT_T_INFO] = { "info", NULL, VEXT_IN },
	[VIRTIO_EXTENT_T_RESIZED] = { "resized", vext_args_resized, .writes = true },
	[VIRTIO_EXTENT_T_FREEZE] = { "freeze" },
	[VIRTIO_EXTENT_T_ADVANCE] = { "advance", NULL, VEXT_IN },
	[VIRTIO_EXTENT_T_LET_GO] = { "let_go", vext_args_epoch },
	[VIRTIO_EXTENT_T_PIN] = { "pin", vext_args_id },
	[VIRTIO_EXTENT_T_UNPIN] = { "unpin", vext_args_id },
	/* 15 was writable DAX's SEALED: no type. */
	[VIRTIO_EXTENT_T_PRIVATE] = { "private", vext_args_private, .writes = true },
	/* fxdax: the guest's stats, its text, which a follower's device takes too (TASK-111.4). */
	[VIRTIO_EXTENT_T_STATS] = { "stats", vext_args_stats, VEXT_OUT },
};

/* The row of `req`'s type, or NULL if it has none. */
static const struct vext_type *vext_type(const struct vext_request *req)
{
	if (req->type > VIRTIO_EXTENT_T_LAST || !vext_types[req->type].name)
		return NULL;
	return &vext_types[req->type];
}

/* Whether `req` has `flag`, which none is had always. */
static bool vext_has(const struct vext_request *req, u32 flag)
{
	return !flag || req->flags & flag;
}

int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req)
{
	const struct vext_type *type = vext_type(req);
	struct scatterlist hdr, args, root, resp, *sgs[5];
	unsigned int out_sgs = 0, in_sgs = 0, descs;
	u32 args_len, data_len, root_len;
	struct vext_queue *queue;
	unsigned long flags;
	int err;

	if (!type)
		return -EINVAL;
	/* fxdax: a follower's filesystem writes nothing: a write here is its bug (doc-8, testing 1). */
	if (WARN_ON_ONCE(type->writes && virtio_extent_read_only(vext)))
		return -EROFS;
	data_len = type->data && vext_has(req, type->data_if) ? req->data_len : 0;
	root_len = type->root_if && req->flags & type->root_if ? req->root_len : 0;
	if (data_len && !req->data)
		return -EINVAL;
	/* The root is in one piece: kmalloc'd. */
	if (root_len && (!req->root || is_vmalloc_addr(req->root)))
		return -EINVAL;
	args_len = type->args ? type->args(req, data_len, root_len) : 0;
	req->hdr = (struct virtio_extent_req) {
		.type = cpu_to_le32(req->type),
		.flags = cpu_to_le32(req->flags),
		.unique = cpu_to_le64(atomic64_inc_return(&vext->unique)),
	};
	req->resp.status = 0;
	memset(&req->data_sgt, 0, sizeof(req->data_sgt));
	if (data_len) {
		err = vext_map_data(&req->data_sgt, req->data, data_len);
		if (err)
			return err;
	}

	/* What the device reads: the header, arguments, data and root; then what it writes. */
	sg_init_one(&hdr, &req->hdr, sizeof(req->hdr));
	sgs[out_sgs++] = &hdr;
	if (args_len) {
		sg_init_one(&args, &req->args, args_len);
		sgs[out_sgs++] = &args;
	}
	if (data_len && type->data == VEXT_OUT)
		sgs[out_sgs++] = req->data_sgt.sgl;
	if (root_len) {
		sg_init_one(&root, req->root, root_len);
		sgs[out_sgs++] = &root;
	}
	if (data_len && type->data == VEXT_IN)
		sgs[out_sgs + in_sgs++] = req->data_sgt.sgl;
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
		req->cpu = raw_smp_processor_id();
		spin_lock_irqsave(&queue->lock, flags);
		/*
		 * fxdax: closed, as the device resets or is closed, it has no queues, or will not
		 * once this is added: looked at once, without the lock, before, a request added after
		 * went to a queue let go of, or that a reset device never completed (TASK-201).
		 */
		err = queue->closed ? -ESHUTDOWN :
			virtqueue_add_sgs(queue->vq, sgs, out_sgs, in_sgs, req, GFP_ATOMIC);
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
		/* The queue is full: wait for the device to complete a request, or the queue to close. */
		wait_event(queue->room,
			   READ_ONCE(queue->closed) || READ_ONCE(queue->vq->num_free) >= descs);
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
		if (req->cpu != raw_smp_processor_id())
			atomic64_inc(&queue->cross);
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
 * fxdax: how long a caller spins, and how recent requests must have taken for it to: nested
 * virtualization takes a device's requests past VEXT_SPIN_NS, so none spins, and each pays a halt
 * and a wakeup (TASK-146.1).
 */
static unsigned long vext_spin_ns = VEXT_SPIN_NS;
module_param_named(spin_ns, vext_spin_ns, ulong, 0644);
MODULE_PARM_DESC(spin_ns, "How long a caller polls for its request before it sleeps, in ns");

/*
 * Waits for `req`, whose completion is `done`: on the CPU first, if requests of its type
 * recently took less than vext_spin_ns, and no other task wants the CPU. fxdax: polling its
 * queue meanwhile, of which the device signals no completions: each interrupt costs exits to
 * read and acknowledge it, which a request done in time saves (TASK-91.15).
 */
static void vext_wait(struct virtio_extent *vext, struct vext_request *req, struct completion *done)
{
	struct vext_queue *queue = req->queue;
	u64 spin_ns = READ_ONCE(vext_spin_ns);
	u64 deadline = local_clock() + spin_ns;
	unsigned int spins = 0, reaped;
	unsigned long flags;
	bool completed;

	if (READ_ONCE(vext->recent_ns[req->type]) >= spin_ns) {
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

	/*
	 * fxdax: as it opens again, the same queues, their counts going on: their locks, which a
	 * request takes to find a queue closed, stay as they are (TASK-201).
	 */
	if (!vext->queues) {
		virtio_cread_le(vdev, struct virtio_extent_config, num_request_queues, &offered);
		vext->nr_queues = clamp(offered, 1U, nr_cpu_ids);
		vext->queues = devm_kcalloc(&vdev->dev, vext->nr_queues, sizeof(*vext->queues),
					    GFP_KERNEL);
		if (!vext->queues)
			return -ENOMEM;
		for (i = 0; i < vext->nr_queues; i++) {
			spin_lock_init(&vext->queues[i].lock);
			init_waitqueue_head(&vext->queues[i].room);
		}
	}
	infos = kcalloc(vext->nr_queues, sizeof(*infos), GFP_KERNEL);
	vqs = kcalloc(vext->nr_queues, sizeof(*vqs), GFP_KERNEL);
	err = -ENOMEM;
	if (!infos || !vqs)
		goto out;
	for (i = 0; i < vext->nr_queues; i++)
		infos[i] = (struct virtqueue_info){ "requests", vext_done };
	err = virtio_find_vqs(vdev, vext->nr_queues, vqs, infos, NULL);
	if (err)
		goto out;
	for (i = 0; i < vext->nr_queues; i++) {
		struct vext_queue *queue = &vext->queues[i];

		spin_lock_irq(&queue->lock);
		queue->vq = vqs[i];
		queue->closed = false;
		spin_unlock_irq(&queue->lock);
	}
out:
	kfree(infos);
	kfree(vqs);
	return err;
}

/*
 * Shows `counters`, one per request type, each on a line of its own after the type's name. fxdax:
 * not the numbers no type has (vext_types).
 */
static ssize_t request_counters_show(char *buf, atomic64_t *counters)
{
	ssize_t len = 0;
	int type;

	for (type = 0; type <= VIRTIO_EXTENT_T_LAST; type++) {
		if (vext_types[type].name)
			len += sysfs_emit_at(buf, len, "%s %lld\n", vext_types[type].name,
					     atomic64_read(&counters[type]));
	}
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
 * completed polling them, without one (vext_wait), all told; and the requests completed on a CPU
 * other than their own (TASK-276).
 */
static ssize_t completions_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;
	s64 interrupts = 0, polled = 0, cross = 0;
	unsigned int i;

	for (i = 0; i < vext->nr_queues; i++) {
		interrupts += atomic64_read(&vext->queues[i].interrupts);
		polled += atomic64_read(&vext->queues[i].polled);
		cross += atomic64_read(&vext->queues[i].cross);
	}
	return sysfs_emit(buf, "interrupts %lld\npolled %lld\ncross %lld\n", interrupts, polled,
			  cross);
}
static DEVICE_ATTR_RO(completions);

/*
 * fxdax: the `window` attribute: the DAX window's length, 0 if the device has none, how much of it
 * has struct pages, and the time making them took (TASK-92.8).
 */
static ssize_t window_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct virtio_extent *vext = dev_to_virtio(dev)->priv;
	struct vext_window *window = &vext->window;

	return sysfs_emit(buf, "len %llu\nmade %lld\nmade_ns %lld\n", window->len,
			  atomic64_read(&window->made), atomic64_read(&window->made_ns));
}
static DEVICE_ATTR_RO(window);

static struct attribute *vext_attrs[] = {
	&dev_attr_requests.attr,
	&dev_attr_request_ns.attr,
	&dev_attr_queues.attr,
	&dev_attr_completions.attr,
	&dev_attr_window.attr,
	NULL,
};

static const struct attribute_group vext_attr_group = { .attrs = vext_attrs };

/*
 * fxdax: the protocol version this driver speaks (TASK-141.3): VIRTIO_EXTENT_VERSION. Another,
 * set for a test, has it refuse the device as it would a device of another version.
 */
static uint vext_version = VIRTIO_EXTENT_VERSION;
module_param_named(version, vext_version, uint, 0444);
MODULE_PARM_DESC(version, "The device protocol version this driver speaks: for tests alone");

/*
 * fxdax: 0 if the device is of the protocol version this driver speaks (TASK-141.3); otherwise
 * -EPROTONOSUPPORT, both versions logged. Read first, as nothing else of the configuration may
 * mean, in another version, what it does in this one.
 */
static int vext_check_version(struct virtio_device *vdev)
{
	u32 version;

	virtio_cread_le(vdev, struct virtio_extent_config, version, &version);
	if (version == vext_version)
		return 0;
	dev_err(&vdev->dev,
		"the device is of protocol version %u, this driver of %u: refused, as a driver of another build is\n",
		version, vext_version);
	return -EPROTONOSUPPORT;
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
	virtio_cread_le(vdev, struct virtio_extent_config, max_segments, &config->max_segments);
	virtio_cread_le(vdev, struct virtio_extent_config, max_root, &config->max_root);
	virtio_cread_le(vdev, struct virtio_extent_config, branch, &config->branch);
	virtio_cread_le(vdev, struct virtio_extent_config, max_extent, &config->max_extent);
	/* fxdax: set as the device opens, and never changed (doc-13). */
	config->private_max = 0;
	if (virtio_has_feature(vdev, VIRTIO_EXTENT_F_DAX))
		virtio_cread_le(vdev, struct virtio_extent_config, private_max, &config->private_max);
}

#if IS_ENABLED(CONFIG_FS_DAX)
/* The window's part holding its byte at `offset`. */
static unsigned int vext_window_part(const struct vext_window *window, u64 offset)
{
	return ((window->phys + offset) >> PA_SECTION_SHIFT) - (window->phys >> PA_SECTION_SHIFT);
}

/* Makes the struct pages of the window's part `part`, with its lock held: 0, or a -errno. */
static int vext_window_make_part(struct vext_window *window, unsigned int part)
{
	struct dev_pagemap *pgmap = &window->parts[part];
	u64 start = ktime_get_ns();
	unsigned int nofs;
	void *kaddr;

	/* Its allocations reclaim nothing of a filesystem, whose locks the caller may hold. */
	nofs = memalloc_nofs_save();
	kaddr = memremap_pages(pgmap, window->nid);
	memalloc_nofs_restore(nofs);
	if (IS_ERR(kaddr))
		return PTR_ERR(kaddr);
	WARN_ON_ONCE(kaddr != window->kaddr + (pgmap->range.start - window->phys));
	atomic64_add(range_len(&pgmap->range), &window->made);
	atomic64_add(ktime_get_ns() - start, &window->made_ns);
	/* Pairs with vext_window_make's test_bit_acquire: a part seen made has its pages. */
	smp_mb__before_atomic();
	set_bit(part, window->made_parts);
	return 0;
}

/*
 * Makes the struct pages of the window's parts holding its `len` bytes from `offset`, if they
 * have none yet: 0, or a -errno. Sleeps, if it makes one.
 */
static int vext_window_make(struct vext_window *window, u64 offset, u64 len)
{
	unsigned int part = vext_window_part(window, offset);
	unsigned int last = vext_window_part(window, offset + len - 1);
	int err = 0;

	for (; part <= last && !err; part++) {
		if (test_bit_acquire(part, window->made_parts))
			continue;
		mutex_lock(&window->lock);
		if (!test_bit(part, window->made_parts))
			err = vext_window_make_part(window, part);
		mutex_unlock(&window->lock);
	}
	return err;
}

/*
 * The window's memory at page `pgoff` of it, up to `nr_pages` of it, as virtio_fs's. fxdax: their
 * struct pages made first, if they have none (TASK-92.8): the caller may sleep.
 */
static long vext_dax_direct_access(struct dax_device *dax_dev, pgoff_t pgoff, long nr_pages,
				   enum dax_access_mode mode, void **kaddr, unsigned long *pfn)
{
	struct vext_window *window = dax_get_private(dax_dev);
	phys_addr_t offset = PFN_PHYS(pgoff);
	long pages = min_t(long, nr_pages, window->len / PAGE_SIZE - pgoff);
	int err;

	might_sleep();
	if (pages <= 0)
		return pages;
	err = vext_window_make(window, offset, PFN_PHYS(pages));
	if (err)
		return err;
	if (kaddr)
		*kaddr = window->kaddr + offset;
	if (pfn)
		*pfn = PHYS_PFN(window->phys + offset);
	return pages;
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

/* Lets the window's made parts go, after its dax_device: unbinding, nothing uses them. */
static void vext_window_unmake(void *data)
{
	struct vext_window *window = data;
	unsigned int part;

	for_each_set_bit(part, window->made_parts, window->nr_parts)
		memunmap_pages(&window->parts[part]);
}

/*
 * Sets the window up, `region`, as pages a dax_device hands out, which mappings of files can map,
 * their struct pages made as they are reached: 0, or a -errno.
 */
static int vext_map_window_dax(struct device *dev, struct vext_window *window,
			       const struct virtio_shm_region *region)
{
	u64 end = region->addr + region->len;
	struct dax_device *dax;
	unsigned int part;
	int err;

	window->phys = region->addr;
	window->len = region->len;
	/* Where memremap_pages maps each part of a pgmap of FS_DAX's. */
	window->kaddr = __va(region->addr);
	window->nid = dev_to_node(dev);
	window->nr_parts = vext_window_part(window, region->len - 1) + 1;
	window->parts = devm_kcalloc(dev, window->nr_parts, sizeof(*window->parts), GFP_KERNEL);
	window->made_parts = devm_bitmap_zalloc(dev, window->nr_parts, GFP_KERNEL);
	if (!window->parts || !window->made_parts)
		return -ENOMEM;
	mutex_init(&window->lock);
	for (part = 0; part < window->nr_parts; part++) {
		u64 section = (region->addr >> PA_SECTION_SHIFT) + part;
		struct dev_pagemap *pgmap = &window->parts[part];

		pgmap->type = MEMORY_DEVICE_FS_DAX;
		pgmap->range = (struct range) {
			.start = max_t(u64, region->addr, section << PA_SECTION_SHIFT),
			.end = min_t(u64, end, (section + 1) << PA_SECTION_SHIFT) - 1,
		};
		pgmap->nr_range = 1;
	}
	/* Released after the dax_device's put, below: no direct access is left then. */
	err = devm_add_action_or_reset(dev, vext_window_unmake, window);
	if (err)
		return err;
	dax = alloc_dax(window, &vext_dax_ops);
	if (IS_ERR(dax))
		return PTR_ERR(dax);
	window->dax = dax;
	return devm_add_action_or_reset(dev, vext_put_dax, dax);
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
	return vext_map_window_dax(&vdev->dev, &vext->window, &region);
}
#endif

static void vext_stats_work(struct work_struct *work);

static int vext_probe(struct virtio_device *vdev)
{
	struct virtio_extent *vext;
	int err;

	err = vext_check_version(vdev);
	if (err)
		return err;
	vext = devm_kzalloc(&vdev->dev, sizeof(*vext), GFP_KERNEL);
	if (!vext)
		return -ENOMEM;
	vext->vdev = vdev;
	vdev->priv = vext;
	spin_lock_init(&vext->changed_lock);
	mutex_init(&vext->open_lock);
	mutex_init(&vext->stats_lock);
	INIT_WORK(&vext->stats_work, vext_stats_work);
	vext_read_config(vext);
#if IS_ENABLED(CONFIG_FS_DAX)
	err = vext_map_window(vext);
	if (err)
		return err;
#endif
	err = vext_find_queues(vext);
	if (err)
		return err;
	/* A request's data pages, beside its header, arguments and response, fit a queue. */
	vext->config.max_segments = min(vext->config.max_segments,
					virtqueue_get_vring_size(vext->queues[0].vq) - 3);
	virtio_device_ready(vdev);
	/*
	 * fxdax: the device activates as it takes DRIVER_OK, and asks for a reset if it can't: as it
	 * refuses a driver that didn't take every feature it offers (TASK-141.3). It serves nothing
	 * then, which the driver would wait for for good.
	 */
	if (vdev->config->get_status(vdev) & VIRTIO_CONFIG_S_NEEDS_RESET) {
		dev_err(&vdev->dev, "the device refused its activation, which the VMM's log says why\n");
		err = -ENODEV;
	} else {
		err = sysfs_create_group(&vdev->dev.kobj, &vext_attr_group);
		if (!err) {
			err = vext_selftest_add(vext, vdev);
			if (err)
				sysfs_remove_group(&vdev->dev.kobj, &vext_attr_group);
		}
	}
	if (err) {
		virtio_reset_device(vdev);
		vdev->config->del_vqs(vdev);
		return err;
	}
	mutex_lock(&devices_lock);
	list_add(&vext->node, &devices);
	mutex_unlock(&devices_lock);
	dev_info(&vdev->dev, "extent store \"%s\", branch %llx, %u request queue%s%s%s\n",
		 vext->config.tag, vext->config.branch, vext->nr_queues,
		 vext->nr_queues == 1 ? "" : "s",
		 vext->window.dax ? ", with a DAX window copying on write" : "",
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

void virtio_extent_set_stats(struct virtio_extent *vext, vext_stats_t stats, void *arg)
{
	mutex_lock(&vext->stats_lock);
	vext->stats = stats;
	vext->stats_arg = arg;
	mutex_unlock(&vext->stats_lock);
}
EXPORT_SYMBOL_GPL(virtio_extent_set_stats);

/*
 * fxdax: answers the host's latest ask for the guest's stats, if it hasn't (F_STATS, TASK-111.4):
 * with what the filesystem says (virtio_extent_set_stats), or nothing. Answered once, failing or
 * not: the host asks again, which answers the asks before it too.
 */
static void vext_stats_work(struct work_struct *work)
{
	struct virtio_extent *vext = container_of(work, struct virtio_extent, stats_work);
	struct vext_request *req = NULL;
	ssize_t len = 0;
	char *buf = NULL;
	u64 asked, again;
	int err;

	/* Read whole: the same twice, as the device may change it between halves. */
	virtio_cread_le(vext->vdev, struct virtio_extent_config, stats_asked, &asked);
	do {
		again = asked;
		virtio_cread_le(vext->vdev, struct virtio_extent_config, stats_asked, &asked);
	} while (asked != again);
	if (!asked || asked == vext->stats_answered)
		return;
	vext->stats_answered = asked;
	req = kzalloc(sizeof(*req), GFP_KERNEL);
	buf = kvmalloc(VIRTIO_EXTENT_MAX_STATS, GFP_KERNEL);
	if (!req || !buf) {
		err = -ENOMEM;
		goto out;
	}
	mutex_lock(&vext->stats_lock);
	if (vext->stats)
		len = vext->stats(vext->stats_arg, buf, VIRTIO_EXTENT_MAX_STATS);
	mutex_unlock(&vext->stats_lock);
	if (len < 0) {
		dev_warn_ratelimited(&vext->vdev->dev, "the filesystem's stats: %zd\n", len);
		len = 0;
	}
	req->type = VIRTIO_EXTENT_T_STATS;
	req->offset = asked;
	req->data = buf;
	req->data_len = min_t(size_t, len, VIRTIO_EXTENT_MAX_STATS);
	err = virtio_extent_call(vext, req);
out:
	/* A follower's device, closed, answers itself. */
	if (err && err != -ESHUTDOWN)
		dev_warn_ratelimited(&vext->vdev->dev, "answering an ask for stats: %d\n", err);
	kvfree(buf);
	kfree(req);
}

static void vext_config_changed(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;
	unsigned long flags;

	spin_lock_irqsave(&vext->changed_lock, flags);
	if (vext->changed)
		vext->changed(vext->changed_arg);
	spin_unlock_irqrestore(&vext->changed_lock, flags);
	/* fxdax: an ask for the guest's stats, if it changed, the work finds (TASK-111.4). */
	if (vext_stats_asked(vext))
		schedule_work(&vext->stats_work);
}

/*
 * fxdax: closes its queues to requests, under each one's lock, which a request is added under
 * (virtio_extent_submit): none is added after, and those waiting for room find them closed.
 */
static void vext_close_queues(struct virtio_extent *vext)
{
	unsigned int i;

	for (i = 0; i < vext->nr_queues; i++) {
		struct vext_queue *queue = &vext->queues[i];

		spin_lock_irq(&queue->lock);
		queue->closed = true;
		spin_unlock_irq(&queue->lock);
		wake_up_all(&queue->room);
	}
}

/*
 * fxdax: resets the device, and lets its queues go (virtio_extent_close): a filesystem that
 * unmounted left only what no task waits for, its last let-go of commits, which fails. Its queues
 * closed first, then the stats work, which asks of the device whenever the host asks for stats,
 * done: its request in flight answered by the device, live still, and any after refused
 * (-ESHUTDOWN). Then the reset, which nothing adds to a queue after: a request the stats work
 * added once the reset had let go of the queues was a use after free, or, added once it had
 * completed those queued, it waited for ever (TASK-201).
 */
static int vext_reset_prepare(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;
	struct vext_request *req;
	unsigned int i;

	vext_close_queues(vext);
	flush_work(&vext->stats_work);
	virtio_reset_device(vdev);
	for (i = 0; i < vext->nr_queues; i++) {
		struct vext_queue *queue = &vext->queues[i];

		for (;;) {
			spin_lock_irq(&queue->lock);
			req = virtqueue_detach_unused_buf(queue->vq);
			spin_unlock_irq(&queue->lock);
			if (!req)
				break;
			req->status = -EIO;
			sg_free_table(&req->data_sgt);
			req->done(req);
		}
	}
	vdev->config->del_vqs(vdev);
	return 0;
}

/* fxdax: finds its queues again (virtio_extent_open): the core then sets DRIVER_OK. */
static int vext_reset_done(struct virtio_device *vdev)
{
	return vext_find_queues(vdev->priv);
}

void virtio_extent_close(struct virtio_extent *vext)
{
	mutex_lock(&vext->open_lock);
	if (virtio_extent_read_only(vext) && !vext->closed) {
		WARN_ON_ONCE(virtio_device_reset_prepare(vext->vdev));
		vext->closed = true;
	}
	mutex_unlock(&vext->open_lock);
}
EXPORT_SYMBOL_GPL(virtio_extent_close);

int virtio_extent_open(struct virtio_extent *vext)
{
	struct virtio_device *vdev = vext->vdev;
	int err = 0;

	mutex_lock(&vext->open_lock);
	if (!vext->closed)
		goto out;
	err = virtio_device_reset_done(vdev);
	if (err)
		goto out;
	/*
	 * The device opens the store as it takes DRIVER_OK. Failing to, it asks for a reset, which
	 * leaves it closed, for a later call: FAILED, which an error here would set, stays.
	 */
	if (vdev->config->get_status(vdev) & VIRTIO_CONFIG_S_NEEDS_RESET) {
		err = virtio_device_reset_prepare(vdev) ?: -EBUSY;
		goto out;
	}
	virtio_cread_le(vdev, struct virtio_extent_config, branch, &vext->config.branch);
	vext->closed = false;
out:
	mutex_unlock(&vext->open_lock);
	return err;
}
EXPORT_SYMBOL_GPL(virtio_extent_open);

static void vext_remove(struct virtio_device *vdev)
{
	struct virtio_extent *vext = vdev->priv;

	mutex_lock(&devices_lock);
	list_del(&vext->node);
	mutex_unlock(&devices_lock);
	/*
	 * fxdax: a filesystem goes on using the device, whose window it maps, and which goes with
	 * this: unbinding waits for it to unmount, which needs requests to go on (TASK-79.48).
	 */
	wait_event_idle(devices_unused, !atomic_read(&vext->users));
	/* fxdax: no ask for stats is answered from now on (TASK-111.4). */
	cancel_work_sync(&vext->stats_work);
	vext_selftest_remove(vdev);
	sysfs_remove_group(&vdev->dev.kobj, &vext_attr_group);
	/* Closed, it has no queues left to let go of. */
	if (!vext->closed)
		vext_reset_prepare(vdev);
}

static const struct virtio_device_id vext_ids[] = {
	{ VIRTIO_ID_EXTENT, VIRTIO_DEV_ANY_ID },
	{ 0 },
};

static const unsigned int vext_features[] = {
#if IS_ENABLED(CONFIG_FS_DAX)
	VIRTIO_EXTENT_F_DAX,
#endif
	VIRTIO_EXTENT_F_WRITE_COMMIT,
	VIRTIO_EXTENT_F_RESIZE,
	VIRTIO_EXTENT_F_FREEZE,
	VIRTIO_EXTENT_F_READ_ONLY,
	VIRTIO_EXTENT_F_FREE_MANY,
	VIRTIO_EXTENT_F_MAP_MANY,
	VIRTIO_EXTENT_F_MAP_AHEAD,
	VIRTIO_EXTENT_F_STATS,
};

static struct virtio_driver vext_driver = {
	.driver.name = "virtio_extent",
	/* fxdax: its window's memmap built beside the rest of the boot (virtio_extent_find). */
	.driver.probe_type = PROBE_PREFER_ASYNCHRONOUS,
	.id_table = vext_ids,
	.feature_table = vext_features,
	.feature_table_size = ARRAY_SIZE(vext_features),
	.probe = vext_probe,
	.remove = vext_remove,
	.config_changed = vext_config_changed,
	.reset_prepare = vext_reset_prepare,
	.reset_done = vext_reset_done,
};

module_virtio_driver(vext_driver);
MODULE_DEVICE_TABLE(virtio, vext_ids);
MODULE_DESCRIPTION("The virtio extent-store device's driver");
MODULE_LICENSE("Dual BSD/GPL");
