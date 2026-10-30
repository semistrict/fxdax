// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The extent-store device, as the filesystem uses it (see device.h): requests of the driver's,
 * each allocated as it is submitted and freed as it completes, and the gate between stores
 * through the DAX window's writable mappings and the requests that seal what they map.
 */

#include <linux/lockdep.h>
#include <linux/minmax.h>
#include <linux/moduleparam.h>
#include <linux/seq_file.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/timekeeping.h>
#include <linux/virtio_extent.h>
#include <linux/wait.h>

#include "device.h"
#include "hist.h"

/* fxdax: the gate's class, as lockdep has it (gate_map). */
static struct lock_class_key fxfs_device_gate_key;

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
	/*
	 * fxdax: the gate as lockdep sees it, a read-write semaphore's: a store holds it shared, from
	 * fxfs_device_store_begin to fxfs_device_store_end (not recursively: a store entering it
	 * again would wait for a seal waiting for itself), and a seal takes it whole as it waits for
	 * the stores. So lockdep learns what the gate is taken around, a seal waiting for stores
	 * that wait for what the sealer holds among it, and fxfs_device_assert_in_gate has something
	 * to check (TASK-44.9). Nothing without lockdep.
	 */
	struct lockdep_map gate_map;
	fxfs_device_sealer_t sealer;
	fxfs_device_sealed_t sealed;
	void *sealer_arg;
	/* fxdax: what the window's requests took, and the stores seals held off (TASK-91.7). */
	atomic64_t maps, map_ns, unmaps, unmap_ns, stores_held, stores_held_ns;
	/* fxdax: what MAP requests took, and UNMAPs, each (TASK-111.6). */
	struct fxfs_hist map_hist, unmap_hist;
	/* fxdax: the requests the maps took, fewer than the maps with MAP_MANY (TASK-91.4). */
	atomic64_t map_requests;
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
	lockdep_init_map(&dev->gate_map, "fxfs_device_gate", &fxfs_device_gate_key, 0);
	config = virtio_extent_config(vext);
	/* Memory not aligned to pages spans one more page than its length needs. */
	max_io = min_t(u64, config->max_transfer, (u64)(config->max_segments - 1) * PAGE_SIZE);
	*info = (struct fxfs_device_info) {
		.block_size = config->block_size,
		.max_io = rounddown(max_io, config->block_size),
		.max_root = config->max_root,
		.flags = (virtio_extent_write_commit(vext) ? FXFS_DEVICE_WRITE_COMMIT : 0) |
			 (virtio_extent_freeze(vext) ? FXFS_DEVICE_FREEZE : 0) |
			 (virtio_extent_read_only(vext) ? FXFS_DEVICE_READ_ONLY : 0) |
			 (virtio_extent_free_many(vext) ? FXFS_DEVICE_FREE_MANY : 0),
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
	u64 seals, held = 0;

	spin_lock(&dev->lock);
	while (dev->sealing) {
		spin_unlock(&dev->lock);
		held = held ?: ktime_get_ns();
		wait_event(dev->wait, !READ_ONCE(dev->sealing));
		spin_lock(&dev->lock);
	}
	if (held) {
		atomic64_inc(&dev->stores_held);
		atomic64_add(ktime_get_ns() - held, &dev->stores_held_ns);
	}
	dev->stores++;
	seals = dev->seals;
	spin_unlock(&dev->lock);
	rwsem_acquire_read(&dev->gate_map, 0, 0, _RET_IP_);
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
	if (entered)
		rwsem_acquire_read(&dev->gate_map, 0, 1, _RET_IP_);
	return entered;
}

void fxfs_device_wait_unsealed(struct fxfs_device *dev)
{
	wait_event(dev->wait, !READ_ONCE(dev->sealing));
}

void fxfs_device_store_end(struct fxfs_device *dev)
{
	bool last;

	rwsem_release(&dev->gate_map, _RET_IP_);
	spin_lock(&dev->lock);
	last = !--dev->stores && dev->sealing;
	spin_unlock(&dev->lock);
	if (last)
		wake_up_all(&dev->wait);
}

void fxfs_device_assert_in_gate(struct fxfs_device *dev)
{
#ifdef CONFIG_LOCKDEP
	lockdep_assert(lock_is_held(&dev->gate_map));
#endif
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
	/* Whole, as a seal takes it waiting for the stores, until they have left it. */
	rwsem_acquire(&dev->gate_map, 0, 0, _RET_IP_);
	wait_event(dev->wait, !READ_ONCE(dev->stores));
	rwsem_release(&dev->gate_map, _RET_IP_);
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

/*
 * fxdax: diagnostic (TASK-121): whether maps carry MAP_AHEAD, which has the VM process map the
 * pages for the guest ahead of its accesses, after the MAP, not waited for.
 */
static bool fxfs_device_maps_ahead = true;
module_param_named(map_ahead, fxfs_device_maps_ahead, bool, 0644);
MODULE_PARM_DESC(map_ahead, "Ask the device to map pages ahead of the guest's accesses (diagnostic)");

/* fxdax: the flags of a map, `writable` and accessed next if `ahead`, that the device takes. */
static u32 fxfs_device_map_flags(struct fxfs_device *dev, bool writable, bool ahead)
{
	u32 flags = writable ? VIRTIO_EXTENT_MAP_WRITE : 0;

	if (ahead && READ_ONCE(fxfs_device_maps_ahead) && virtio_extent_map_ahead(dev->vext))
		flags |= VIRTIO_EXTENT_MAP_AHEAD;
	return flags;
}

int fxfs_device_map(struct fxfs_device *dev, u64 branch, u64 sequence, u64 offset, u64 len,
		    u64 window_offset, bool writable, bool ahead)
{
	struct vext_request *req = kzalloc(sizeof(*req), GFP_NOFS);
	u64 began;
	int err;

	if (!req)
		return -ENOMEM;
	req->type = VIRTIO_EXTENT_T_MAP;
	req->flags = fxfs_device_map_flags(dev, writable, ahead);
	req->id = (struct vext_id) { branch, sequence };
	req->offset = offset;
	req->len = len;
	req->window_offset = window_offset;
	began = ktime_get_ns();
	err = virtio_extent_call(dev->vext, req);
	atomic64_inc(&dev->maps);
	atomic64_inc(&dev->map_requests);
	atomic64_add(ktime_get_ns() - began, &dev->map_ns);
	fxfs_hist_add(&dev->map_hist, ktime_get_ns() - began);
	kfree(req);
	return err;
}

/* fxdax: the most maps one MAP_MANY request holds: its further ones a page's worth. */
#define FXFS_DEVICE_MAP_MANY (PAGE_SIZE / sizeof(struct virtio_extent_map_entry) + 1)

/* fxdax: `count` maps, from `maps`, in one MAP_MANY request; `count` at most FXFS_DEVICE_MAP_MANY. */
static int fxfs_device_map_one_request(struct fxfs_device *dev, const struct fxfs_device_map *maps,
				       size_t count, struct virtio_extent_map_entry *entries)
{
	struct vext_request *req = kzalloc(sizeof(*req), GFP_NOFS);
	u64 began;
	size_t i;
	int err;

	if (!req)
		return -ENOMEM;
	for (i = 1; i < count; i++)
		entries[i - 1] = (struct virtio_extent_map_entry) {
			.map = {
				.id = { cpu_to_le64(maps[i].branch), cpu_to_le64(maps[i].sequence) },
				.offset = cpu_to_le64(maps[i].offset),
				.len = cpu_to_le64(maps[i].len),
				.window_offset = cpu_to_le64(maps[i].window_offset),
			},
			.flags = cpu_to_le32(fxfs_device_map_flags(dev, maps[i].writable, maps[i].ahead)),
		};
	req->type = VIRTIO_EXTENT_T_MAP;
	req->flags = VIRTIO_EXTENT_MAP_MANY | fxfs_device_map_flags(dev, maps[0].writable, maps[0].ahead);
	req->id = (struct vext_id) { maps[0].branch, maps[0].sequence };
	req->offset = maps[0].offset;
	req->len = maps[0].len;
	req->window_offset = maps[0].window_offset;
	req->data = entries;
	req->data_len = (count - 1) * sizeof(*entries);
	began = ktime_get_ns();
	err = virtio_extent_call(dev->vext, req);
	atomic64_add(count, &dev->maps);
	atomic64_inc(&dev->map_requests);
	atomic64_add(ktime_get_ns() - began, &dev->map_ns);
	fxfs_hist_add(&dev->map_hist, ktime_get_ns() - began);
	kfree(req);
	return err;
}

int fxfs_device_map_many(struct fxfs_device *dev, const struct fxfs_device_map *maps,
			 size_t count)
{
	struct virtio_extent_map_entry *entries;
	size_t done, n;
	int err = 0;

	if (count == 1 || !virtio_extent_map_many(dev->vext)) {
		for (done = 0; done < count && !err; done++)
			err = fxfs_device_map(dev, maps[done].branch, maps[done].sequence,
					      maps[done].offset, maps[done].len,
					      maps[done].window_offset, maps[done].writable,
					      maps[done].ahead);
		return err;
	}
	entries = kmalloc_array(FXFS_DEVICE_MAP_MANY - 1, sizeof(*entries), GFP_NOFS);
	if (!entries)
		return -ENOMEM;
	for (done = 0; done < count && !err; done += n) {
		n = min_t(size_t, count - done, FXFS_DEVICE_MAP_MANY);
		err = fxfs_device_map_one_request(dev, maps + done, n, entries);
	}
	kfree(entries);
	return err;
}

/*
 * fxdax: an UNMAP's request is allocated without failing: the window space it unmaps goes to other
 * extents once it is unmapped, and is lost to the window if it can't be (TASK-44.22). A request is
 * a few hundred bytes, which the allocator makes room for. It can't be on the stack, whose memory
 * the device can't be given if it is vmalloc's (VMAP_STACK).
 */
int fxfs_device_unmap(struct fxfs_device *dev, u64 window_offset, u64 len)
{
	struct vext_request *req = kzalloc(sizeof(*req), GFP_NOFS | __GFP_NOFAIL);
	u64 began;
	int err;

	req->type = VIRTIO_EXTENT_T_UNMAP;
	req->len = len;
	req->window_offset = window_offset;
	began = ktime_get_ns();
	err = virtio_extent_call(dev->vext, req);
	atomic64_inc(&dev->unmaps);
	atomic64_add(ktime_get_ns() - began, &dev->unmap_ns);
	fxfs_hist_add(&dev->unmap_hist, ktime_get_ns() - began);
	kfree(req);
	return err;
}

void fxfs_device_show_stats(struct fxfs_device *dev, struct seq_file *m)
{
	seq_printf(m, "window_maps %lld\nwindow_maps_ns %lld\n", atomic64_read(&dev->maps),
		   atomic64_read(&dev->map_ns));
	seq_printf(m, "window_map_requests %lld\n", atomic64_read(&dev->map_requests));
	fxfs_hist_show(m, "window_map_requests", &dev->map_hist);
	seq_printf(m, "window_unmaps %lld\nwindow_unmaps_ns %lld\n", atomic64_read(&dev->unmaps),
		   atomic64_read(&dev->unmap_ns));
	fxfs_hist_show(m, "window_unmaps", &dev->unmap_hist);
	seq_printf(m, "stores_held_by_seals %lld\nstores_held_by_seals_ns %lld\n",
		   atomic64_read(&dev->stores_held), atomic64_read(&dev->stores_held_ns));
}
