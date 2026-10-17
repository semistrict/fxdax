// SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause
// Copyright 2026 The fxdax Authors.
/*
 * The driver's self-test (selftest.h). It mints extents past the device's next sequence number,
 * so it can run on a store holding a filesystem, though it leaves its extents and root there.
 */

#include <linux/atomic.h>
#include <linux/completion.h>
#include <linux/device.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/virtio.h>
#include <linux/virtio_extent.h>

#include "selftest.h"

/* How many extents the test writes at once, and their blocks. */
#define EXTENTS 32
#define EXTENT_BLOCKS 2
#define ROOT "fxdax extent store self-test"

struct vext_selftest {
	struct virtio_extent *vext;
	struct mutex lock;
	char result[256];
};

/* Requests in flight, and their completion once all are done. */
struct batch {
	atomic_t pending;
	struct completion done;
};

static void batch_done(struct vext_request *req)
{
	struct batch *batch = req->private;

	if (atomic_dec_and_test(&batch->pending))
		complete(&batch->done);
}

/* Submits `count` requests at once, and waits for them all. */
static int submit_all(struct virtio_extent *vext, struct vext_request *reqs, int count)
{
	struct batch batch;
	int i, err;

	atomic_set(&batch.pending, count);
	init_completion(&batch.done);
	for (i = 0; i < count; i++) {
		reqs[i].done = batch_done;
		reqs[i].private = &batch;
		err = virtio_extent_submit(vext, &reqs[i]);
		if (err) {
			/* Those not submitted won't complete. */
			if (atomic_sub_and_test(count - i, &batch.pending))
				complete(&batch.done);
			wait_for_completion(&batch.done);
			return err;
		}
	}
	wait_for_completion(&batch.done);
	return 0;
}

static u8 pattern(int extent, size_t offset)
{
	return (u8)(extent * 31 + offset / 512);
}

static void fill(u8 *buf, size_t len, int extent)
{
	size_t i;

	for (i = 0; i < len; i++)
		buf[i] = pattern(extent, i);
}

static bool matches(const u8 *buf, size_t len, int extent)
{
	size_t i;

	for (i = 0; i < len; i++)
		if (buf[i] != pattern(extent, i))
			return false;
	return true;
}

#define FAIL(...)                                                         \
	do {                                                              \
		snprintf(st->result, sizeof(st->result), __VA_ARGS__); \
		goto out;                                                 \
	} while (0)

static void run(struct vext_selftest *st)
{
	struct virtio_extent *vext = st->vext;
	const struct vext_config *config = virtio_extent_config(vext);
	size_t len = EXTENT_BLOCKS * config->block_size;
	struct virtio_extent_info *info = NULL;
	struct virtio_extent_root_len *root_len;
	struct vext_request *reqs, *req;
	u8 *bufs[EXTENTS] = { NULL };
	u8 *root = NULL;
	u64 next, used, window_len;
	u8 *window;
	int i, err;

	reqs = kcalloc(EXTENTS, sizeof(*reqs), GFP_KERNEL);
	info = kzalloc(sizeof(*info), GFP_KERNEL);
	root = kzalloc(config->max_root + sizeof(*root_len), GFP_KERNEL);
	if (!reqs || !info || !root)
		FAIL("no memory");
	for (i = 0; i < EXTENTS; i++) {
		bufs[i] = kmalloc(len, GFP_KERNEL);
		if (!bufs[i])
			FAIL("no memory");
	}
	req = &reqs[0];
#define CALL(...)                                                       \
	({                                                              \
		memset(req, 0, sizeof(*req));                           \
		*req = (struct vext_request) { __VA_ARGS__ };           \
		virtio_extent_call(vext, req);                          \
	})
#define EXTENT(i) ((struct vext_id) { config->branch, next + (i) })

	err = CALL(.type = VIRTIO_EXTENT_T_INFO, .data = info, .data_len = sizeof(*info));
	if (err)
		FAIL("INFO: %d", err);
	next = le64_to_cpu(info->next_sequence);
	used = le64_to_cpu(info->used);

	/* Many writes in flight. */
	memset(reqs, 0, EXTENTS * sizeof(*reqs));
	for (i = 0; i < EXTENTS; i++) {
		fill(bufs[i], len, i);
		reqs[i] = (struct vext_request) {
			.type = VIRTIO_EXTENT_T_WRITE, .id = EXTENT(i), .len = len, .data = bufs[i],
		};
	}
	err = submit_all(vext, reqs, EXTENTS);
	for (i = 0; !err && i < EXTENTS; i++)
		err = reqs[i].status;
	if (err)
		FAIL("WRITE: %d", err);

	memcpy(root, ROOT, sizeof(ROOT) - 1);
	err = CALL(.type = VIRTIO_EXTENT_T_COMMIT,
		   .flags = VIRTIO_EXTENT_COMMIT_ROOT | VIRTIO_EXTENT_COMMIT_DURABLE,
		   .data = root, .data_len = sizeof(ROOT) - 1);
	if (err)
		FAIL("COMMIT: %d", err);
	memset(root, 0, config->max_root + sizeof(*root_len));
	err = CALL(.type = VIRTIO_EXTENT_T_ROOT, .data = root,
		   .data_len = config->max_root + sizeof(*root_len));
	root_len = (void *)(root + config->max_root);
	if (err || le32_to_cpu(root_len->len) != sizeof(ROOT) - 1 ||
	    memcmp(root, ROOT, sizeof(ROOT) - 1))
		FAIL("ROOT: %d, %u bytes", err, le32_to_cpu(root_len->len));

	/* Many reads in flight. */
	memset(reqs, 0, EXTENTS * sizeof(*reqs));
	for (i = 0; i < EXTENTS; i++) {
		memset(bufs[i], 0, len);
		reqs[i] = (struct vext_request) {
			.type = VIRTIO_EXTENT_T_READ, .id = EXTENT(i), .len = len, .data = bufs[i],
		};
	}
	err = submit_all(vext, reqs, EXTENTS);
	for (i = 0; !err && i < EXTENTS; i++) {
		err = reqs[i].status;
		if (!err && !matches(bufs[i], len, i))
			FAIL("READ: extent %d differs", i);
	}
	if (err)
		FAIL("READ: %d", err);

	/* The commit sealed them. */
	err = CALL(.type = VIRTIO_EXTENT_T_WRITE, .id = EXTENT(0), .len = len, .data = bufs[0]);
	if (err != -EPERM)
		FAIL("WRITE of a sealed extent: %d", err);
	fill(bufs[0], len, EXTENTS);
	err = CALL(.type = VIRTIO_EXTENT_T_WRITE, .flags = VIRTIO_EXTENT_WRITE_BARRIER,
		   .id = EXTENT(EXTENTS), .len = len, .data = bufs[0]);
	if (err)
		FAIL("WRITE with a barrier: %d", err);

	/* Read in place, with no copy, through the DAX window. */
	window = virtio_extent_window(vext, &window_len);
	if (window) {
		size_t mapped = ALIGN(len, config->map_alignment);

		err = CALL(.type = VIRTIO_EXTENT_T_MAP, .id = EXTENT(1), .len = mapped);
		if (err)
			FAIL("MAP: %d", err);
		if (!matches(window, len, 1) || memchr_inv(window + len, 0, mapped - len))
			FAIL("the DAX window doesn't hold extent 1");
		err = CALL(.type = VIRTIO_EXTENT_T_UNMAP, .len = mapped);
		if (err)
			FAIL("UNMAP: %d", err);
	}

	err = CALL(.type = VIRTIO_EXTENT_T_FREE, .id = EXTENT(2));
	if (err)
		FAIL("FREE: %d", err);
	err = CALL(.type = VIRTIO_EXTENT_T_READ, .id = EXTENT(2), .len = len, .data = bufs[1]);
	if (err != -ENOENT)
		FAIL("READ of a freed extent: %d", err);
	err = CALL(.type = VIRTIO_EXTENT_T_INFO, .data = info, .data_len = sizeof(*info));
	if (err || le64_to_cpu(info->used) != used + EXTENTS * len)
		FAIL("INFO: %d, %llu bytes used", err, le64_to_cpu(info->used));
	/* What followed the last commit is durable once another commits it. */
	err = CALL(.type = VIRTIO_EXTENT_T_COMMIT, .flags = VIRTIO_EXTENT_COMMIT_DURABLE);
	if (err)
		FAIL("COMMIT: %d", err);

	snprintf(st->result, sizeof(st->result), "ok%s", window ? "" : ", without DAX");
out:
	for (i = 0; i < EXTENTS; i++)
		kfree(bufs[i]);
	kfree(root);
	kfree(info);
	kfree(reqs);
}

static ssize_t selftest_show(struct device *dev, struct device_attribute *attr, char *buf)
{
	struct vext_selftest *st = dev_get_drvdata(dev);
	ssize_t len;

	mutex_lock(&st->lock);
	len = sysfs_emit(buf, "%s\n", st->result);
	mutex_unlock(&st->lock);
	return len;
}

static ssize_t selftest_store(struct device *dev, struct device_attribute *attr,
			      const char *buf, size_t count)
{
	struct vext_selftest *st = dev_get_drvdata(dev);

	mutex_lock(&st->lock);
	run(st);
	mutex_unlock(&st->lock);
	return count;
}

static DEVICE_ATTR_RW(selftest);

int vext_selftest_add(struct virtio_extent *vext, struct virtio_device *vdev)
{
	struct vext_selftest *st;

	st = devm_kzalloc(&vdev->dev, sizeof(*st), GFP_KERNEL);
	if (!st)
		return -ENOMEM;
	st->vext = vext;
	mutex_init(&st->lock);
	strscpy(st->result, "not run", sizeof(st->result));
	dev_set_drvdata(&vdev->dev, st);
	return device_create_file(&vdev->dev, &dev_attr_selftest);
}

void vext_selftest_remove(struct virtio_device *vdev)
{
	device_remove_file(&vdev->dev, &dev_attr_selftest);
}
