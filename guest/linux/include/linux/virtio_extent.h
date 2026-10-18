/* SPDX-License-Identifier: GPL-2.0 OR BSD-3-Clause */
/* Copyright 2026 The fxdax Authors. */
/*
 * The virtio extent-store device (fxdax's backlog doc-4, which specifies it), and the guest
 * kernel's driver of it (drivers/virtio_extent), which the guest's Fxfs stores its filesystem
 * through: extents it names, written once and sealed by the next commit, freed whole, a root
 * naming where its filesystem starts, and a DAX window that sealed extents map into.
 */
#ifndef _LINUX_VIRTIO_EXTENT_H
#define _LINUX_VIRTIO_EXTENT_H

#include <linux/types.h>

#define VIRTIO_ID_EXTENT 0x4658 /* "FX" */

/* Shared memory region 0 is a DAX window, and MAP and UNMAP work. */
#define VIRTIO_EXTENT_F_DAX 0
/* MAP maps open extents writably. */
#define VIRTIO_EXTENT_F_DAX_WRITE 1
/* WRITE takes WRITE_COMMIT: a COMMIT follows the write, in one request. */
#define VIRTIO_EXTENT_F_WRITE_COMMIT 2
/*
 * The configuration holds the store's capacity, which the host changes while the guest runs,
 * telling it with a configuration change; a shrink the guest is asked to take (asked) it answers
 * with RESIZED.
 */
#define VIRTIO_EXTENT_F_RESIZE 3
/*
 * FREEZE tells the device the filesystem is frozen, or thawed, and INFO says the branch the
 * guest mints extents of: a snapshot of the VM is taken frozen, and a VM restored from it, whose
 * store is a fork of its own, learns its branch as its filesystem thaws (fxdax's TASK-39.1).
 */
#define VIRTIO_EXTENT_F_FREEZE 4
/*
 * The device follows a store another VM writes, read-only (fxdax's doc-8): ADVANCE, LET_GO, PIN
 * and UNPIN work, and the configuration's `committed` changes as the writer commits, telling the
 * guest with a configuration change. The driver sends nothing that writes.
 */
#define VIRTIO_EXTENT_F_READ_ONLY 5

#define VIRTIO_EXTENT_T_READ 1
#define VIRTIO_EXTENT_T_WRITE 2
#define VIRTIO_EXTENT_T_FREE 3
#define VIRTIO_EXTENT_T_MAP 4
#define VIRTIO_EXTENT_T_UNMAP 5
#define VIRTIO_EXTENT_T_COMMIT 6
#define VIRTIO_EXTENT_T_ROOT 7
#define VIRTIO_EXTENT_T_INFO 8
#define VIRTIO_EXTENT_T_RESIZED 9
#define VIRTIO_EXTENT_T_FREEZE 10
/* F_READ_ONLY: the follower's view moves to the writer's latest commit, and says its epoch. */
#define VIRTIO_EXTENT_T_ADVANCE 11
/*
 * F_READ_ONLY: the view lets go of what the commits before the one an epoch names held, which an
 * ADVANCE said: served after the next ADVANCE, it lets go of no more.
 */
#define VIRTIO_EXTENT_T_LET_GO 12
/* F_READ_ONLY: keeps an extent the view holds, whatever commit it advances to, until UNPIN. */
#define VIRTIO_EXTENT_T_PIN 13
#define VIRTIO_EXTENT_T_UNPIN 14
/* The last request type. */
#define VIRTIO_EXTENT_T_LAST VIRTIO_EXTENT_T_UNPIN

/* WRITE: a barrier precedes it; a COMMIT follows it (F_WRITE_COMMIT); that commit is durable. */
#define VIRTIO_EXTENT_WRITE_BARRIER (1 << 0)
#define VIRTIO_EXTENT_WRITE_COMMIT (1 << 1)
#define VIRTIO_EXTENT_WRITE_DURABLE (1 << 2)
/* COMMIT: it completes once durable; it sets the root. */
#define VIRTIO_EXTENT_COMMIT_DURABLE (1 << 0)
#define VIRTIO_EXTENT_COMMIT_ROOT (1 << 1)
/* MAP: writable. */
#define VIRTIO_EXTENT_MAP_WRITE (1 << 0)
/* RESIZED: the guest took the size it was asked to shrink to. */
#define VIRTIO_EXTENT_RESIZED_TOOK (1 << 0)
/*
 * FREEZE: the filesystem is frozen, nothing open, and it writes nothing until it thaws; without
 * it, thawed.
 */
#define VIRTIO_EXTENT_FREEZE_FROZEN (1 << 0)

struct virtio_extent_config {
	__u8 tag[36];
	__le32 num_request_queues;
	__le32 block_size;
	__le32 map_alignment;
	__le32 max_transfer;
	__le32 max_segments;
	__le32 max_root;
	__le64 branch;
	__le64 max_extent;
	/* F_RESIZE: the store's capacity, and the shrink the guest is asked to take, or 0. */
	__le64 capacity;
	__le64 asked;
	/* F_READ_ONLY: the epoch of the writer's latest commit. */
	__le64 committed;
} __packed;

struct virtio_extent_id {
	__le64 branch;
	__le64 sequence;
};

struct virtio_extent_req {
	__le32 type;
	__le32 flags;
	__le64 unique;
};

struct virtio_extent_resp {
	__le32 status;
	__le32 reserved;
};

struct virtio_extent_io {
	struct virtio_extent_id id;
	__le64 offset;
	__le32 len;
	__le32 reserved;
};

struct virtio_extent_map {
	struct virtio_extent_id id;
	__le64 offset;
	__le64 len;
	__le64 window_offset;
};

struct virtio_extent_unmap {
	__le64 window_offset;
	__le64 len;
};

struct virtio_extent_commit {
	__le32 root_len;
	__le32 reserved;
};

struct virtio_extent_root_len {
	__le32 len;
	__le32 reserved;
};

/* RESIZED: the capacity the guest was asked to shrink to. */
struct virtio_extent_resized {
	__le64 capacity;
};

/* ADVANCE: the epoch of the commit the view is as of now. LET_GO: the epoch it lets go up to. */
struct virtio_extent_epoch {
	__le64 epoch;
	__le64 reserved;
};

struct virtio_extent_info {
	__le64 capacity;
	__le64 used;
	__le64 available;
	__le64 next_sequence;
	/* The lowest sequence number to mint from: its writer's range's start. */
	__le64 sequence_floor;
	/* F_FREEZE: the branch the guest mints extents of, its store's. */
	__le64 branch;
};

#ifdef __KERNEL__

#include <linux/scatterlist.h>

/* A bound device. */
struct virtio_extent;
struct dax_device;

/* An extent's ID, in the CPU's byte order. */
struct vext_id {
	u64 branch;
	u64 sequence;
};

/* The device's configuration, in the CPU's byte order. */
struct vext_config {
	char tag[37];
	u32 block_size;
	u32 map_alignment;
	u32 max_transfer;
	/* The most pages one request's data spans: the device's limit, or the queue's. */
	u32 max_segments;
	u32 max_root;
	u64 branch;
	u64 max_extent;
};

/*
 * A request. The caller fills in the request's part, and the device's part is the driver's.
 * Requests and their data must be memory the device can address: kmalloc'd, or vmalloc'd for
 * data, not on the stack.
 */
struct vext_request {
	/* The request: one of VIRTIO_EXTENT_T_*, and its flags. */
	u32 type;
	u32 flags;
	/* READ, WRITE, FREE, MAP, PIN, UNPIN: the extent. */
	struct vext_id id;
	/*
	 * READ, WRITE, MAP: where in the extent. MAP, UNMAP: how much, and where in the window.
	 * RESIZED: the capacity asked (offset). LET_GO: the epoch (offset).
	 */
	u64 offset;
	u64 len;
	u64 window_offset;
	/*
	 * The data: READ's buffer, of len bytes; WRITE's data, of len bytes; COMMIT's root, of
	 * data_len bytes, with VIRTIO_EXTENT_COMMIT_ROOT; ROOT's buffer, of max_root bytes and a
	 * struct virtio_extent_root_len after them; INFO's struct virtio_extent_info; ADVANCE's
	 * struct virtio_extent_epoch.
	 */
	void *data;
	u32 data_len;
	/* Called once the device completes the request, from its interrupt: it must not sleep. */
	void (*done)(struct vext_request *req);
	void *private;
	/* 0, or the device's -errno, once done. */
	int status;

	/* The device's part. */
	struct virtio_extent_req hdr;
	/* When it was queued, for the device's `request_ns`. */
	u64 queued_ns;
	union {
		struct virtio_extent_io io;
		struct virtio_extent_id id;
		struct virtio_extent_map map;
		struct virtio_extent_unmap unmap;
		struct virtio_extent_commit commit;
		struct virtio_extent_resized resized;
		struct virtio_extent_epoch epoch;
	} args;
	struct virtio_extent_resp resp;
	struct sg_table data_sgt;
};

/* The bound device tagged `tag`, or NULL. It stays bound while the filesystem uses it. */
struct virtio_extent *virtio_extent_find(const char *tag);
const struct vext_config *virtio_extent_config(struct virtio_extent *vext);

/*
 * Submits `req`, whose `done` the driver calls once the device completes it. Waits for room in
 * the queue if it is full. Returns 0, or a -errno if it couldn't submit it, when it doesn't call
 * `done`: -EINVAL if its data spans more than max_segments pages.
 * Sleeps, so it is called from a task's context.
 */
int virtio_extent_submit(struct virtio_extent *vext, struct vext_request *req);
/* Submits `req` and waits for the device to complete it, returning its status. */
int virtio_extent_call(struct virtio_extent *vext, struct vext_request *req);

/* The DAX window, mapped in the kernel, and its length, or NULL if the device has none. */
void *virtio_extent_window(struct virtio_extent *vext, u64 *len);
/* The DAX window's dax_device, with FS_DAX, or NULL. */
struct dax_device *virtio_extent_dax(struct virtio_extent *vext);
/* Whether the device has a DAX window that maps open extents writably (DAX_WRITE). */
bool virtio_extent_dax_write(struct virtio_extent *vext);
/* Whether a WRITE may carry WRITE_COMMIT (F_WRITE_COMMIT). */
bool virtio_extent_write_commit(struct virtio_extent *vext);
/* Whether the device takes FREEZE, and INFO says the branch. */
bool virtio_extent_freeze(struct virtio_extent *vext);
/*
 * Whether the device follows a store another VM writes, read-only (F_READ_ONLY): the driver
 * refuses what would write (-EROFS), and the filesystem mounts read-only.
 */
bool virtio_extent_read_only(struct virtio_extent *vext);
/* F_READ_ONLY: the epoch of the writer's latest commit, as the configuration says now. */
u64 virtio_extent_committed(struct virtio_extent *vext);

/*
 * The store's capacity, and the shrink the guest is asked to take, or 0, as the configuration
 * says now: false if the device doesn't say (F_RESIZE).
 */
bool virtio_extent_capacity(struct virtio_extent *vext, u64 *capacity, u64 *asked);
/*
 * Has the driver call `changed(arg)` as the device's configuration changes, its capacity or a
 * shrink asked, or its writer's latest commit; or none, if `changed` is NULL. Called from the device's interrupt: it must not
 * sleep.
 */
typedef void (*vext_config_changed_t)(void *arg);
void virtio_extent_set_config_changed(struct virtio_extent *vext, vext_config_changed_t changed,
				      void *arg);

#endif /* __KERNEL__ */

#endif
